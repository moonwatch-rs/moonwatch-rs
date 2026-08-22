//! The worker thread: sample the active window, record the events, write them out.
//!
//! This runs on its own thread because the main thread has to be given over to the platform
//! UI event loop that the tray icon needs. It is the sole owner of the event buffer, and
//! requests reach it as [`MoonwatcherSignal`]s from the tray, from OS signals, and (on
//! Windows) from the session-end window messages.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossbeam_channel::{Receiver, Sender};
use log::{debug, error, info, warn};

use crate::core::model::config::Config;
use crate::daemon::status::{RecordingState, SharedStatus};
use crate::daemon::tray::SharedOutputDir;
use crate::recorder::recorder::EventRecorder;
use crate::sampler;
use crate::sampler::desktop::Desktop;
use crate::sampler::sampler::{sample_active_window, sampling_problem, SampleOutcome};

/// One-shot channel handed to the worker along with a request, so that the requester can
/// wait until the request has actually been carried out.
pub type Ack = Sender<()>;

#[derive(Debug)]
pub enum MoonwatcherSignal {
    ReloadConfig,
    /// Stop (or resume) sampling without terminating the process.
    SetPaused(bool),
    /// Flush buffered events to disk right now, without terminating.
    WriteNow { done: Option<Ack> },
    /// Flush buffered events to disk and exit the worker loop.
    Terminate { done: Option<Ack> },
}

/// Handle for talking to the worker thread that owns the event buffer.
///
/// Held by the UI thread (tray menu callbacks, and on Windows the session-end window
/// messages) and by the OS signal handlers; none of them own the [`EventRecorder`].
/// `terminate_and_wait` is what makes a clean logout possible: it asks the worker to
/// flush and blocks until it confirms, so we do not return from `WM_QUERYENDSESSION`
/// before the data is on disk.
#[derive(Clone)]
pub struct WorkerHandle {
    signals: Sender<MoonwatcherSignal>,
}

impl WorkerHandle {
    pub fn new(signals: Sender<MoonwatcherSignal>) -> WorkerHandle {
        WorkerHandle { signals }
    }

    /// Send a signal to the worker without waiting for it to be handled.
    ///
    /// Failures are logged, not fatal: this is called from OS signal handlers and window
    /// procedures, where a panic would be much worse than a dropped signal, and the only
    /// realistic failure is the worker having already exited.
    pub fn send(&self, signal: MoonwatcherSignal) {
        if let Err(undelivered) = self.signals.send(signal) {
            log::warn!("Could not deliver {:?} to worker thread, it is no longer running",
                       undelivered.into_inner());
        }
    }

    /// Ask the worker to flush its buffer, then wait for it to confirm.
    pub fn flush_and_wait(&self, timeout: Duration) -> bool {
        self.request_and_wait(|done| MoonwatcherSignal::WriteNow { done: Some(done) }, timeout)
    }

    /// Ask the worker to flush its buffer and exit, then wait for it to confirm.
    ///
    /// Returns `true` if the worker acknowledged (or had already exited), `false` on
    /// timeout. Safe to call repeatedly: once the worker is gone the request cannot even
    /// be sent, so later calls return immediately instead of blocking.
    pub fn terminate_and_wait(&self, timeout: Duration) -> bool {
        self.request_and_wait(|done| MoonwatcherSignal::Terminate { done: Some(done) }, timeout)
    }

    fn request_and_wait(&self,
                        make_signal: impl FnOnce(Ack) -> MoonwatcherSignal,
                        timeout: Duration) -> bool {
        let (ack_sender, ack_receiver) = crossbeam_channel::bounded(1);
        if self.signals.send(make_signal(ack_sender)).is_err() {
            // The worker has already exited, so it has already done its final write.
            return true;
        }

        match ack_receiver.recv_timeout(timeout) {
            Ok(()) => true,
            // Worker dropped the ack without sending: it is on its way out anyway.
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => true,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                error!("Worker thread did not respond within {timeout:?}, data may be lost");
                false
            }
        }
    }
}

/// A loaded configuration together with the desktop implementation that goes with it.
struct Active {
    config: Config,
    desktop: Box<dyn Desktop>,
    sample_every: Duration,
    write_every: Duration,
}

/// Read `main_config.json` and set up everything derived from it.
fn load_active(config_path: &Path) -> Result<Active> {
    // The context is what the user reads in the tray, so lead with the file name rather
    // than the full path - the message has to survive being clipped into a tooltip.
    let name = config_path.file_name()
        .unwrap_or(config_path.as_os_str())
        .to_string_lossy()
        .into_owned();

    let config = Config::from_file(config_path)
        .with_context(|| format!("{name} could not be loaded"))?;
    info!("Read configuration: {config:?}");

    // A non-positive period would panic `crossbeam_channel::tick`, so it is rejected here
    // and shown in the tray like any other configuration mistake.
    let sample_every = positive_period(config.main_config.sample_every_sec, "sampleEverySec")
        .with_context(|| format!("{name} could not be used"))?;
    let write_every = positive_period(config.main_config.write_every_sec, "writeEverySec")
        .with_context(|| format!("{name} could not be used"))?;

    let desktop = sampler::get_desktop().context("no usable desktop implementation")?;
    info!("Using desktop implementation: {}", desktop.implementation_name());
    desktop.before_main_loop_start()?;

    Ok(Active { config, desktop, sample_every, write_every })
}

fn positive_period(seconds: i32, name: &str) -> Result<Duration> {
    if seconds <= 0 {
        bail!("{name} must be a positive number of seconds, got {seconds}");
    }
    Ok(Duration::from_secs(seconds as u64))
}

/// What the worker is currently reporting to the tray, aside from the loaded configuration.
struct WorkerState {
    paused: bool,
    /// Why the configuration is unusable. Survives a failed reload, so the previous
    /// configuration can keep recording while the problem is displayed.
    config_problem: Option<String>,
    /// Why sampling is not working. Only ever set for a genuine malfunction.
    sampling_problem: Option<String>,
}

impl WorkerState {
    fn publish(&self, status: &SharedStatus, sample_every: Option<Duration>) {
        let recording = match (sample_every.is_some(), self.paused) {
            (false, _) => RecordingState::Stopped,
            (true, true) => RecordingState::Paused,
            (true, false) => RecordingState::Recording,
        };
        let (config_problem, sampling_problem) =
            (self.config_problem.clone(), self.sampling_problem.clone());

        status.update(|status| {
            status.recording = recording;
            status.config_problem = config_problem;
            status.sampling_problem = sampling_problem;
            status.sample_every = sample_every;
        });
    }
}

/// Why an inner loop returned.
enum Outcome {
    /// A new configuration has been loaded and the previous buffer already written out.
    Reload(Active),
    /// The worker has been asked to stop; the final write has already happened.
    Terminate,
}

/// Sample the active window and write the results out; the heart of the daemon.
///
/// The configuration is loaded here rather than by the caller: a bad `main_config.json` must
/// not stop the daemon, or the user would be left with no tray icon and no way to reload
/// after fixing it. It shows up as a problem state in the tray instead.
pub fn run_worker(config_path: PathBuf,
                  signals: Receiver<MoonwatcherSignal>,
                  status: SharedStatus,
                  output_dir: SharedOutputDir) -> Result<()> {
    let mut state = WorkerState {
        paused: false,
        config_problem: None,
        sampling_problem: None,
    };

    // The initial load goes through the same path as a reload, so a configuration that was
    // already broken at login behaves exactly like one broken while we are running.
    let mut active = load_or_report(&config_path, &output_dir, &mut state);
    state.publish(&status, active.as_ref().map(|active| active.sample_every));

    // TODO do writing in separate thread to not stall sampling

    loop {
        // Taken out of the binding so that `sampling_loop` can borrow the `Config` it owns:
        // the recorder holds that borrow for as long as it buffers events, so the swap can
        // only happen once the inner loop (and its recorder) is gone.
        let outcome = match active.take() {
            None => idle_loop(&config_path, &signals, &status, &output_dir, &mut state)?,
            Some(loaded) => {
                sampling_loop(loaded, &config_path, &signals, &status, &output_dir, &mut state)?
            }
        };

        match outcome {
            Outcome::Reload(loaded) => {
                let sample_every = loaded.sample_every;
                active = Some(loaded);
                state.publish(&status, Some(sample_every));
            }
            Outcome::Terminate => {
                info!("Terminating");
                return Ok(());
            }
        }
    }
}

/// Wait for something to do while no usable configuration is loaded.
///
/// Nothing is sampled and nothing is buffered here, so `WriteNow` has nothing to write - it
/// is still acknowledged, because a session-end handler may be blocked on it.
fn idle_loop(config_path: &Path,
             signals: &Receiver<MoonwatcherSignal>,
             status: &SharedStatus,
             output_dir: &SharedOutputDir,
             state: &mut WorkerState) -> Result<Outcome> {
    loop {
        match signals.recv()? {
            MoonwatcherSignal::ReloadConfig => {
                info!("Reloading configuration file");
                if let Some(loaded) = load_or_report(config_path, output_dir, state) {
                    return Ok(Outcome::Reload(loaded));
                }
                state.publish(status, None);
            }
            MoonwatcherSignal::SetPaused(paused) => {
                if set_paused(state, paused) {
                    state.publish(status, None);
                }
            }
            MoonwatcherSignal::WriteNow { done } => {
                debug!("Nothing to write, no configuration is loaded");
                acknowledge(done);
            }
            MoonwatcherSignal::Terminate { done } => {
                debug!("Nothing to write, no configuration is loaded");
                acknowledge(done);
                return Ok(Outcome::Terminate);
            }
        }
    }
}

/// Sample and record until the configuration is replaced or the worker is asked to stop.
fn sampling_loop(active: Active,
                 config_path: &Path,
                 signals: &Receiver<MoonwatcherSignal>,
                 status: &SharedStatus,
                 output_dir: &SharedOutputDir,
                 state: &mut WorkerState) -> Result<Outcome> {
    let Active { config, desktop, sample_every, write_every } = active;
    let mut recorder = EventRecorder::new(&config);

    let mut sample_tick = crossbeam_channel::tick(sample_every);
    let writer_tick = crossbeam_channel::tick(write_every);
    // Sampling has been backed off, because the screen is locked or sampling failed.
    let mut slow = false;

    loop {
        crossbeam_channel::select! {
            recv(signals) -> sig => {
                match sig? {
                    MoonwatcherSignal::ReloadConfig => {
                        info!("Reloading configuration file");
                        // Loaded before anything is torn down: if it fails, whatever is
                        // already recording keeps recording with the settings it has.
                        match load_or_report(config_path, output_dir, state) {
                            Some(loaded) => {
                                // Written out with the *old* configuration still in effect,
                                // so buffered events land where they were sampled for.
                                write_events(&mut recorder, None);
                                // A reload replaces the desktop implementation, so whatever
                                // was wrong with sampling is no longer known to be wrong.
                                state.sampling_problem = None;
                                return Ok(Outcome::Reload(loaded));
                            }
                            None => state.publish(status, Some(sample_every)),
                        }
                    }
                    MoonwatcherSignal::SetPaused(paused) => {
                        if set_paused(state, paused) {
                            state.publish(status, Some(sample_every));
                        }
                    }
                    MoonwatcherSignal::WriteNow { done } => {
                        write_events(&mut recorder, done);
                    }
                    MoonwatcherSignal::Terminate { done } => {
                        write_events(&mut recorder, done);
                        return Ok(Outcome::Terminate);
                    }
                }
            }
            recv(writer_tick) -> _ => {
                write_events(&mut recorder, None);
            }
            recv(sample_tick) -> _ => {
                if state.paused {
                    continue
                }

                // Not quite accurate while backed off, which is deliberate: crediting the
                // window with ten intervals because the screen was locked would be worse.
                let res = sample_active_window(desktop.as_ref(), sample_every);

                // Report a broken desktop implementation, and clear the report as soon as a
                // sample succeeds. Only publish on a change, so a persistent failure does not
                // write a status line to the log every time round.
                let new_problem = sampling_problem(&res);
                if new_problem != state.sampling_problem {
                    state.sampling_problem = new_problem;
                    state.publish(status, Some(sample_every));
                }

                match res {
                    Ok(SampleOutcome::ScreenLocked) => {
                        if !slow {
                            info!("slowing down sample rate");
                            slow = true;
                            sample_tick = crossbeam_channel::tick(10 * sample_every);
                        }
                    }
                    Ok(SampleOutcome::NoActiveWindow) => {
                        // Deliberately no back-off: unlike a locked screen this usually lasts
                        // a moment, and sampling a tenth as often for the next few minutes
                        // because the user clicked the desktop would lose real data.
                        debug!("Nothing is focused, skipping this sample");
                    }
                    Ok(SampleOutcome::Event(e)) => {
                        if slow {
                            info!("resetting sample rate");
                            slow = false;
                            sample_tick = crossbeam_channel::tick(sample_every);
                        }

                        // Tagging, redaction and dropping happen inside `push`, according to
                        // the recorder configuration.
                        debug!("Recording {e:?}");
                        recorder.push(e);
                    }
                    Err(e) => {
                        warn!("Could not sample the active window: {e:#}");
                        if !slow {
                            info!("slowing down sample rate");
                            slow = true;
                            sample_tick = crossbeam_channel::tick(10 * sample_every);
                        }
                    }
                }
            }
        }
    }
}

/// (Re)load the configuration, recording a description of any failure for the tray.
///
/// On success the problem is cleared and the shared output directory is updated, so the
/// tray's "Open log folder" follows a reload.
fn load_or_report(config_path: &Path,
                  output_dir: &SharedOutputDir,
                  state: &mut WorkerState) -> Option<Active> {
    match load_active(config_path) {
        Ok(active) => {
            set_shared_output_dir(output_dir, &active.config.log_output_subdirectory);
            state.config_problem = None;
            Some(active)
        }
        Err(e) => {
            error!("Could not load configuration: {e:?}");
            // `{:#}` is the whole context chain on one line, which is what fits in a menu
            // item; the multi-line `{:?}` version above is what the log file gets.
            state.config_problem = Some(format!("{e:#}"));
            None
        }
    }
}

/// Apply a pause request, returning whether anything actually changed.
fn set_paused(state: &mut WorkerState, paused: bool) -> bool {
    if paused == state.paused {
        return false;
    }

    info!("{} recording", if paused { "Pausing" } else { "Resuming" });
    state.paused = paused;
    // Nothing is sampling while paused, so there is no failure to report.
    if paused {
        state.sampling_problem = None;
    }
    true
}

/// Flush the buffer and, if someone is waiting on this write, tell them it is done.
///
/// The acknowledgement is sent whatever happens - even when the write failed. The caller is
/// typically the Windows session-end handler, and making it wait out its whole timeout only
/// risks the system killing us mid-shutdown.
fn write_events(recorder: &mut EventRecorder, done: Option<Ack>) {
    match recorder.dump() {
        Ok(Some(path)) => info!("Wrote events to {}", path.display()),
        Ok(None) => debug!("Nothing to write, no events are buffered"),
        Err(e) => error!("Failed to write events, data will be lost!! Error: {e:?}"),
    }

    acknowledge(done);
}

fn acknowledge(done: Option<Ack>) {
    if let Some(done) = done {
        let _ = done.send(());
    }
}

fn set_shared_output_dir(shared: &Mutex<Option<PathBuf>>, output_dir: &Path) {
    let mut shared = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *shared = Some(output_dir.to_path_buf());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moonwatch-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn state() -> WorkerState {
        WorkerState { paused: false, config_problem: None, sampling_problem: None }
    }

    /// A `WriteNow`/`Terminate` request must always be acknowledged, so that a waiting
    /// session-end handler is released instead of sitting out its timeout.
    #[test]
    fn a_write_with_nothing_buffered_is_still_acknowledged() {
        let dir = temp_dir();
        let config_path = dir.join("main_config.json");
        fs::write(&config_path, VALID_MAIN_CONFIG).expect("write config");
        let config = Config::from_file(&config_path).expect("config should load");
        let mut recorder = EventRecorder::new(&config);

        let (ack, acked) = crossbeam_channel::bounded(1);
        write_events(&mut recorder, Some(ack));

        assert_eq!(acked.try_recv(), Ok(()));
        assert!(!config.log_output_subdirectory.exists(),
                "an empty write should not create the output dir");

        fs::remove_dir_all(&dir).ok();
    }

    /// Shutting down while main_config.json is broken must not hang the session-end handler:
    /// the idle loop has no recorder at all, and still has to answer.
    #[test]
    fn terminating_without_a_configuration_is_acknowledged() {
        let (signals, signal_chan) = crossbeam_channel::bounded(1);
        let (ack, acked) = crossbeam_channel::bounded(1);
        signals.send(MoonwatcherSignal::Terminate { done: Some(ack) }).expect("send");

        let status = SharedStatus::new();
        let output_dir: SharedOutputDir = Arc::new(Mutex::new(None));
        let outcome = idle_loop(Path::new("missing.json"), &signal_chan, &status,
                               &output_dir, &mut state()).expect("idle loop");

        assert!(matches!(outcome, Outcome::Terminate));
        assert_eq!(acked.try_recv(), Ok(()));
    }

    /// A broken main_config.json must be a reportable problem rather than something that
    /// stops the daemon, and the message has to name the file and the syntax error so the
    /// user can act on what the tray shows them.
    #[test]
    fn load_active_reports_a_malformed_config_instead_of_panicking() {
        let dir = temp_dir();
        let config_path = dir.join("main_config.json");
        fs::write(&config_path, BROKEN_MAIN_CONFIG).expect("write config");

        // .err() rather than .expect_err(): Active holds a Box<dyn Desktop>, so it is not Debug.
        let error = load_active(&config_path).err().expect("malformed JSON should not load");
        let message = format!("{error:#}");

        assert!(message.contains("main_config.json could not be loaded"), "got {message:?}");
        assert!(message.contains("line"), "should point at the syntax error: {message:?}");

        fs::remove_dir_all(&dir).ok();
    }

    /// A period of zero would panic the ticker, so it has to be rejected as a configuration
    /// problem - which is something the user can see and fix.
    #[test]
    fn a_non_positive_sampling_period_is_reported_as_a_configuration_problem() {
        let dir = temp_dir();
        let config_path = dir.join("main_config.json");
        fs::write(&config_path, VALID_MAIN_CONFIG.replace(r#""sampleEverySec": 1"#,
                                                          r#""sampleEverySec": 0"#))
            .expect("write config");

        let error = load_active(&config_path).err().expect("a zero period should be rejected");
        let message = format!("{error:#}");

        assert!(message.contains("sampleEverySec"), "got {message:?}");
        assert!(message.contains("could not be used"), "got {message:?}");

        fs::remove_dir_all(&dir).ok();
    }

    const VALID_MAIN_CONFIG: &str = r#"{
        "logDirectory": "./logs",
        "logOutputSubdirectory": ".",
        "sampleEverySec": 1,
        "writeEverySec": 3600,
        "recorderConfigPath": null,
        "pipelineConfigPath": null,
        "pipelineOutputDirectory": "./output",
        "pipelineOutputFormat": "parquet"
    }"#;

    const BROKEN_MAIN_CONFIG: &str = r#"{ "logDirectory": "./logs", }"#;

    /// End-to-end tests of the worker and the status it publishes.
    ///
    /// Windows only: the unix `Desktop` implementation shells out to `xdotool` and
    /// `xprintidle` against a live X11 session, which CI does not have, so anything here
    /// that expects sampling to actually start would fail there for unrelated reasons.
    #[cfg(windows)]
    mod worker {
        use super::*;
        use std::thread;
        use std::time::Instant;
        use crate::daemon::status::{MoonwatcherStatus, StatusIcon};

        struct Fixture {
            dir: PathBuf,
            config_path: PathBuf,
            signals: Sender<MoonwatcherSignal>,
            status: SharedStatus,
            output_dir: SharedOutputDir,
            worker: Option<thread::JoinHandle<Result<()>>>,
        }

        impl Fixture {
            fn start(initial_config: &str) -> Fixture {
                let dir = temp_dir();
                let config_path = dir.join("main_config.json");
                fs::write(&config_path, initial_config).expect("write config");

                let (signals, signal_chan) = crossbeam_channel::bounded(10);
                let status = SharedStatus::new();
                let output_dir: SharedOutputDir = Arc::new(Mutex::new(None));

                let worker = thread::spawn({
                    let (config_path, status, output_dir) =
                        (config_path.clone(), status.clone(), Arc::clone(&output_dir));
                    move || run_worker(config_path, signal_chan, status, output_dir)
                });

                Fixture { dir, config_path, signals, status, output_dir, worker: Some(worker) }
            }

            fn write_config(&self, contents: &str) {
                fs::write(&self.config_path, contents).expect("rewrite config");
            }

            fn send(&self, signal: MoonwatcherSignal) {
                self.signals.send(signal).expect("worker should be listening");
            }

            /// Block until the published status satisfies `predicate`, and return it.
            fn wait_for(&self,
                        what: &str,
                        predicate: impl Fn(&MoonwatcherStatus) -> bool) -> MoonwatcherStatus {
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    let status = self.status.get();
                    if predicate(&status) {
                        return status;
                    }
                    assert!(Instant::now() < deadline,
                            "timed out waiting for {what}; status is {status:?}");
                    thread::sleep(Duration::from_millis(20));
                }
            }

            fn log_folder(&self) -> Option<PathBuf> {
                self.output_dir.lock().unwrap().clone()
            }

            /// Shut the worker down the way the session-end handler does, and assert it
            /// acknowledged rather than leaving the caller to time out.
            fn shutdown(mut self) {
                let (ack, acked) = crossbeam_channel::bounded(1);
                self.send(MoonwatcherSignal::Terminate { done: Some(ack) });
                assert!(acked.recv_timeout(Duration::from_secs(5)).is_ok(),
                        "Terminate should be acknowledged");

                self.worker.take().unwrap().join().expect("worker should not panic")
                    .expect("worker should exit cleanly");
                fs::remove_dir_all(&self.dir).ok();
            }
        }

        /// The flow this feature exists for: the user breaks main_config.json, sees the
        /// problem in the tray, fixes it and reloads.
        #[test]
        fn a_failed_reload_is_reported_while_recording_continues() {
            let fixture = Fixture::start(VALID_MAIN_CONFIG);

            let healthy = fixture.wait_for("recording to start", |s| {
                s.recording == RecordingState::Recording
            });
            assert_eq!(healthy.icon(), StatusIcon::Recording);
            assert_eq!(healthy.menu_line(), "Recording every 1 s");
            assert!(fixture.log_folder().is_some(), "Open log folder should be usable");

            fixture.write_config(BROKEN_MAIN_CONFIG);
            fixture.send(MoonwatcherSignal::ReloadConfig);

            let broken = fixture.wait_for("the bad config to be reported",
                                          |s| s.config_problem.is_some());
            assert_eq!(broken.icon(), StatusIcon::Problem, "the error icon should win");
            assert_eq!(broken.recording, RecordingState::Recording,
                       "the previous configuration should still be recording");
            let line = broken.menu_line();
            assert!(line.starts_with("main_config.json could not be loaded"), "got {line:?}");
            assert!(line.ends_with("previous settings still in use"), "got {line:?}");

            fixture.write_config(VALID_MAIN_CONFIG);
            fixture.send(MoonwatcherSignal::ReloadConfig);

            let recovered = fixture.wait_for("the problem to clear",
                                             |s| s.config_problem.is_none());
            assert_eq!(recovered.icon(), StatusIcon::Recording);

            fixture.send(MoonwatcherSignal::SetPaused(true));
            let paused = fixture.wait_for("pausing", |s| s.recording == RecordingState::Paused);
            assert_eq!(paused.icon(), StatusIcon::Paused);
            assert_eq!(paused.menu_line(), "Recording paused");

            fixture.shutdown();
        }

        /// A config that was already broken at login used to exit the process before the tray
        /// existed, leaving the user nothing to click.
        #[test]
        fn a_config_broken_at_startup_leaves_the_daemon_running_and_recoverable() {
            let fixture = Fixture::start(BROKEN_MAIN_CONFIG);

            let stopped = fixture.wait_for("the startup failure to be reported", |s| {
                s.config_problem.is_some()
            });
            assert_eq!(stopped.recording, RecordingState::Stopped);
            assert_eq!(stopped.icon(), StatusIcon::Problem);
            assert!(stopped.menu_line()
                        .starts_with("Not recording - main_config.json could not be loaded"),
                    "got {:?}", stopped.menu_line());
            assert!(stopped.sample_every.is_none());
            assert!(fixture.log_folder().is_none(),
                    "Open log folder should be disabled until a config loads");

            fixture.write_config(VALID_MAIN_CONFIG);
            fixture.send(MoonwatcherSignal::ReloadConfig);

            let recovered = fixture.wait_for("recording to start after the fix", |s| {
                s.recording == RecordingState::Recording
            });
            assert!(recovered.config_problem.is_none());
            assert_eq!(recovered.icon(), StatusIcon::Recording);
            assert!(fixture.log_folder().is_some());

            fixture.shutdown();
        }
    }
}
