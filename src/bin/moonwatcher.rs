// No console on Windows, so that logging in at startup does not flash a terminal window.
// Diagnostics go to moonwatcher.log instead, see init_logging().
#![windows_subsystem = "windows"]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use moonwatch_rs::watcher;
use moonwatch_rs::watcher::core::{Ack, ActiveWindowEventV1, Desktop, MoonwatcherSignal, WorkerHandle};
use moonwatch_rs::watcher::config::Config;
use moonwatch_rs::watcher::status::{RecordingState, SharedStatus};
use moonwatch_rs::watcher::tray::{self, SharedOutputDir, TrayContext};
use anyhow::{bail, Context, Result};
use flexi_logger::{Cleanup, Criterion, Duplicate, FileSpec, Logger, LoggerHandle, Naming, WriteMode};
use log::{debug, error, info, warn};
use uuid::Uuid;
use clap::Parser;

/// The outcome of one sample.
///
/// Everything except an `Err` from [`get_window_event`] is a normal outcome. `Err` means the
/// desktop implementation is not working, and is the only case reported to the user.
#[derive(Debug)]
enum ActiveWindowEventResult {
    DesktopLocked,
    /// Nothing is focused. Routine: focus on the desktop, a window being switched, and so on.
    NoActiveWindow,
    Window { e: ActiveWindowEventV1 }
}

fn get_window_event(desktop: &dyn Desktop, duration: Duration) -> Result<ActiveWindowEventResult> {
    if desktop.is_screen_locked() {
        return Ok(ActiveWindowEventResult::DesktopLocked);
    }

    let Some(window) = desktop.get_active_window()? else {
        return Ok(ActiveWindowEventResult::NoActiveWindow);
    };

    let idle_duration = desktop.get_idle_duration()?;
    let window_title = window.get_title().unwrap_or_default();

    // Not being able to read the path is expected for elevated processes, processes owned by
    // another user, and processes that have just exited. We still know the user was active,
    // so record the event without it rather than losing the sample - and do not treat it as
    // the implementation being broken.
    let process_path = match window.get_process_path() {
        Ok(path) => Some(path),
        Err(e) => {
            debug!("Could not determine the process path: {e:#}");
            None
        }
    };

    let e = ActiveWindowEventV1::new(idle_duration, window_title, process_path, duration);
    Ok(ActiveWindowEventResult::Window { e })
}

/// What a sample outcome means for the tray: `Some` only for a genuine malfunction.
///
/// Routine outcomes - a locked screen, nothing focused - must never turn the icon red, or it
/// would be red most of the time for no reason.
fn sampling_problem(result: &Result<ActiveWindowEventResult>) -> Option<String> {
    match result {
        Ok(_) => None,
        Err(e) => Some(format!("{e:#}")),
    }
}

struct MoonwatcherWriter {
    events_to_write: Vec<ActiveWindowEventV1>
}

impl MoonwatcherWriter {
    pub fn new() -> MoonwatcherWriter {
        MoonwatcherWriter {
            events_to_write: vec![]
        }
    }

    pub fn push(&mut self, e: ActiveWindowEventV1) {
        self.events_to_write.push(e)
    }

    pub fn write(&mut self, config: &Config) -> Result<()> {
        if self.events_to_write.is_empty() {
            return Ok(());
        }

        // ensure output dir
        if !config.output_dir.exists() {
            info!("Creating output dir {:?}", config.output_dir);
            fs::create_dir_all(&config.output_dir)?;
        }

        // derive name for output file
        let id = Uuid::now_v7();
        let filename = format!("{id}.jsonl");
        let output_path = config.output_dir.join(filename);

        // TODO consider writing .jsonl.gz instead
        // TODO consider allowing output encryption

        info!("Writing {} events to {:?}", self.events_to_write.len(), output_path);
        let mut fp = fs::File::create(output_path)?;
        while !self.events_to_write.is_empty() {
            let e = self.events_to_write.pop().unwrap();
            let line = e.to_json().to_string();
            fp.write_all(line.as_bytes())?;
            fp.write_all(b"\n")?;
        }

        // This write often happens while the machine is logging off or shutting down, so
        // get the data all the way to the filesystem rather than leaving it in a buffer
        // that a Drop we never reach would have to flush.
        fp.flush()?;
        fp.sync_all()?;

        Ok(())
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// The Moonwatch.rs daemon
struct MoonwatcherCli {
    #[arg(value_name = "CONFIG.JSON", help = "path to config.json file")]
    config_path: PathBuf,

    #[arg(long, help = "run without a system tray icon")]
    no_tray: bool,
}

/// Directory holding config.json, and therefore also moonwatcher.log.
///
/// The Startup shortcut on Windows passes a bare `config.json` with a working directory, so
/// there may be no parent component at all.
fn config_dir(config_path: &Path) -> PathBuf {
    config_path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Set up logging to `moonwatcher.log` next to the configuration file.
///
/// On Windows the daemon has no console, so `println!` output used to be lost entirely,
/// which is exactly what made shutdown problems hard to diagnose. Messages are additionally
/// sent to stderr, where systemd picks them up into the journal on Linux.
///
/// Verbosity can be raised with eg. `MOONWATCH_LOG=debug`.
fn init_logging(config_path: &Path) -> Result<LoggerHandle> {
    let log_dir = config_dir(config_path);

    let spec = std::env::var("MOONWATCH_LOG").unwrap_or_else(|_| "info".to_string());

    let duplicate = if cfg!(debug_assertions) { Duplicate::All } else { Duplicate::Info };

    let logger = Logger::try_with_str(&spec)?
        .log_to_file(FileSpec::default()
            .directory(&log_dir)
            .basename("moonwatcher")
            .suffix("log")
            .suppress_timestamp())
        .rotate(Criterion::Size(2 * 1024 * 1024), Naming::Numbers, Cleanup::KeepLogFiles(3))
        .append()
        // Unbuffered, so that the log still explains what happened if we are killed.
        .write_mode(WriteMode::Direct)
        .format(flexi_logger::detailed_format)
        .duplicate_to_stderr(duplicate)
        .start()?;

    Ok(logger)
}

fn main() -> Result<()> {
    let cli = MoonwatcherCli::parse();
    let config_path = cli.config_path;

    // Held for the lifetime of the process; dropping it shuts the logger down.
    let _logger = init_logging(&config_path)?;

    info!("--- Moonwatch ---");
    info!("Configuration file: {config_path:?}");

    let (signal_sender, signal_receiver) = crossbeam_channel::bounded(100);
    let worker = WorkerHandle::new(signal_sender);
    watcher::install_signal_handlers(worker.clone())?;

    // The configuration is loaded by the worker, not here: a bad config.json must not stop
    // the daemon, or the user would be left with no tray icon and no way to reload after
    // fixing it. It shows up as a problem state in the tray instead.
    let status = SharedStatus::new();
    // Shared so that the tray's "Open log folder" follows a configuration reload.
    let output_dir: SharedOutputDir = Arc::new(Mutex::new(None));

    let worker_thread = {
        let (status, output_dir) = (status.clone(), Arc::clone(&output_dir));
        let config_path = config_path.clone();
        thread::Builder::new()
            .name("moonwatcher-worker".to_string())
            .spawn(move || {
                let result = run_worker(config_path, signal_receiver, status, output_dir);
                if let Err(e) = &result {
                    error!("Watcher stopped with an error: {e:?}");
                }
                // Nothing left for the UI thread to do once the watcher is gone.
                watcher::request_ui_quit();
                result
            })?
    };

    let build_tray = if cli.no_tray {
        info!("Running without a tray icon (--no-tray)");
        None
    } else {
        let context = TrayContext {
            worker: worker.clone(),
            status,
            output_dir,
            config_dir: config_dir(&config_path),
        };
        Some(move || tray::build_tray(context))
    };

    // A missing tray is not fatal: on Linux there may be no display at all, and on Windows
    // the event loop is worth running by itself for the session-end handling.
    if let Err(e) = watcher::run_event_loop(worker.clone(), build_tray) {
        warn!("Could not run the UI event loop, continuing without a tray icon: {e:?}");
    }

    match worker_thread.join() {
        Ok(result) => result,
        Err(_) => bail!("Watcher thread panicked"),
    }
}

/// A loaded configuration together with the desktop implementation that goes with it.
struct Active {
    config: Config,
    desktop: Box<dyn Desktop>,
}

/// Read `config.json` and set up everything derived from it.
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

    let desktop = watcher::get_desktop(&config)
        .context("no usable desktop implementation")?;
    info!("Using desktop implementation: {}", desktop.implementation_name());
    desktop.before_main_loop_start()?;

    Ok(Active { config, desktop })
}

/// What the worker loop samples, and the timers driving it.
///
/// `active` is `None` before the first successful load, which is why the daemon can run at
/// all with a `config.json` the user has broken: the timers are then `never()` receivers and
/// the loop simply waits for a `ReloadConfig`.
struct Sampling {
    active: Option<Active>,
    sample_tick: crossbeam_channel::Receiver<Instant>,
    writer_tick: crossbeam_channel::Receiver<Instant>,
    /// Sampling has been backed off, because the screen is locked or sampling failed.
    slow: bool,
}

impl Sampling {
    fn idle() -> Sampling {
        Sampling {
            active: None,
            sample_tick: crossbeam_channel::never(),
            writer_tick: crossbeam_channel::never(),
            slow: false,
        }
    }

    fn config(&self) -> Option<&Config> {
        self.active.as_ref().map(|active| &active.config)
    }

    fn sample_every(&self) -> Option<Duration> {
        self.config().map(|config| config.sample_every)
    }

    /// (Re)read `config.json` and arm the timers from it.
    ///
    /// On failure whatever was loaded before stays in effect - a daemon that is already
    /// recording keeps recording with the settings it has - and the error is returned so it
    /// can be shown in the tray.
    fn reload(&mut self, config_path: &Path) -> Result<()> {
        let active = load_active(config_path)?;

        self.sample_tick = crossbeam_channel::tick(active.config.sample_every);
        self.writer_tick = crossbeam_channel::tick(active.config.write_every);
        self.slow = false;
        self.active = Some(active);

        Ok(())
    }

    /// Back off sampling: the screen is locked, or sampling just failed.
    fn slow_down(&mut self) {
        let Some(sample_every) = self.sample_every().filter(|_| !self.slow) else {
            return;
        };

        info!("slowing down sample rate");
        self.slow = true;
        self.sample_tick = crossbeam_channel::tick(10 * sample_every);
    }

    fn full_speed(&mut self) {
        let Some(sample_every) = self.sample_every().filter(|_| self.slow) else {
            return;
        };

        info!("resetting sample rate");
        self.slow = false;
        self.sample_tick = crossbeam_channel::tick(sample_every);
    }
}

/// Sample the active window and write the results out; the heart of the daemon.
///
/// Runs on its own thread because the main thread has to be given over to the platform UI
/// event loop that the tray icon needs. It is the sole owner of the event buffer, and
/// requests reach it as [`MoonwatcherSignal`]s from the tray, from OS signals, and (on
/// Windows) from the session-end window messages.
fn run_worker(config_path: PathBuf,
              signal_chan: crossbeam_channel::Receiver<MoonwatcherSignal>,
              status: SharedStatus,
              output_dir: SharedOutputDir) -> Result<()> {
    let mut writer = MoonwatcherWriter::new();
    let mut sampling = Sampling::idle();
    let mut paused = false;
    let mut sampling_failure: Option<String> = None;

    // The initial load goes through the same path as a reload, so a configuration that was
    // already broken at login behaves exactly like one broken while we are running.
    let mut config_failure = reload(&mut sampling, &config_path, &output_dir);
    publish_status(&status, &sampling, paused, &config_failure, &sampling_failure);

    // TODO do writing in separate thread to not stall sampling

    loop {
        // Cloned out so the loop body is free to swap the timers out from under itself;
        // a Receiver clone is an Arc bump, and this runs once per sample at most.
        let (sample_tick, writer_tick) = (sampling.sample_tick.clone(), sampling.writer_tick.clone());

        crossbeam_channel::select! {
            recv(signal_chan) -> sig => {
                match sig? {
                    MoonwatcherSignal::ReloadConfig => {
                        info!("Reloading configuration file");
                        config_failure = reload(&mut sampling, &config_path, &output_dir);
                        // A reload replaces the desktop implementation, so whatever was wrong
                        // with sampling is no longer known to be wrong.
                        sampling_failure = None;
                        publish_status(&status, &sampling, paused, &config_failure, &sampling_failure);
                    }
                    MoonwatcherSignal::SetPaused(new_paused) => {
                        if new_paused != paused {
                            info!("{} recording", if new_paused { "Pausing" } else { "Resuming" });
                            paused = new_paused;
                            // Nothing is sampling while paused, so there is no failure to report.
                            if paused {
                                sampling_failure = None;
                            }
                            publish_status(&status, &sampling, paused, &config_failure, &sampling_failure);
                        }
                    }
                    MoonwatcherSignal::WriteNow { done } => {
                        write_events(&mut writer, sampling.config(), done);
                    }
                    MoonwatcherSignal::Terminate { done } => {
                        write_events(&mut writer, sampling.config(), done);
                        info!("Terminating");
                        break;
                    }
                }
            }
            recv(writer_tick) -> _ => {
                write_events(&mut writer, sampling.config(), None);
            }
            recv(sample_tick) -> _ => {
                if paused {
                    continue
                }
                let Some(active) = sampling.active.as_ref() else {
                    continue
                };

                let res = get_window_event(active.desktop.as_ref(), active.config.sample_every); // this is not quite accurate w/ sampling.slow

                // Report a broken desktop implementation, and clear the report as soon as a
                // sample succeeds. Only publish on a change, so a persistent failure does not
                // write a status line to the log every time round.
                let new_problem = sampling_problem(&res);
                if new_problem != sampling_failure {
                    sampling_failure = new_problem;
                    publish_status(&status, &sampling, paused, &config_failure, &sampling_failure);
                }

                match res {
                    Ok(ActiveWindowEventResult::DesktopLocked) => {
                        sampling.slow_down();
                    }
                    Ok(ActiveWindowEventResult::NoActiveWindow) => {
                        // Deliberately no back-off: unlike a locked screen this usually lasts
                        // a moment, and sampling a tenth as often for the next few minutes
                        // because the user clicked the desktop would lose real data.
                        debug!("Nothing is focused, skipping this sample");
                    }
                    Ok(ActiveWindowEventResult::Window { mut e }) => {
                        sampling.full_speed();

                        let Some(config) = sampling.config() else {
                            continue
                        };

                        // do we want to skip this event?
                        let should_ignore = config.ignore.iter().any(|m| m.matches(&e));
                        if should_ignore {
                            debug!("Ignoring {e:?}");
                            continue
                        };

                        // fill in event according to config
                        e._anonymize = config.anonymize.iter().any(|m| m.matches(&e));
                        for t in &config.tags {
                            if t.matcher.matches(&e) && !e.tags.contains(&t.tag) {
                                e.tags.push_back(t.tag.clone())
                            }
                        }

                        debug!("Recording {e:?}");
                        writer.push(e);
                    }
                    Err(e) => {
                        // Previously swallowed without even a log line, which is how a
                        // permanently broken xdotool could go unnoticed indefinitely.
                        warn!("Could not sample the active window: {e:#}");
                        sampling.slow_down();
                    }
                }
            }
        }
    }

    Ok(())
}

/// (Re)load the configuration, returning a description of the failure for the tray, or
/// `None` if it worked.
fn reload(sampling: &mut Sampling,
          config_path: &Path,
          output_dir: &SharedOutputDir) -> Option<String> {
    match sampling.reload(config_path) {
        Ok(()) => {
            if let Some(config) = sampling.config() {
                set_shared_output_dir(output_dir, &config.output_dir);
            }
            None
        }
        Err(e) => {
            error!("Could not load configuration: {e:?}");
            // `{:#}` is the whole context chain on one line, which is what fits in a menu
            // item; the multi-line `{:?}` version above is what the log file gets.
            Some(format!("{e:#}"))
        }
    }
}

fn publish_status(status: &SharedStatus,
                  sampling: &Sampling,
                  paused: bool,
                  config_failure: &Option<String>,
                  sampling_failure: &Option<String>) {
    let recording = match (sampling.config().is_some(), paused) {
        (false, _) => RecordingState::Stopped,
        (true, true) => RecordingState::Paused,
        (true, false) => RecordingState::Recording,
    };
    let sample_every = sampling.sample_every();
    let (config_failure, sampling_failure) = (config_failure.clone(), sampling_failure.clone());

    status.update(|status| {
        status.recording = recording;
        status.config_problem = config_failure;
        status.sampling_problem = sampling_failure;
        status.sample_every = sample_every;
    });
}

/// Flush the buffer and, if someone is waiting on this write, tell them it is done.
///
/// The acknowledgement is sent whatever happens - even when the write failed, or when there
/// is no configuration to write with. The caller is typically the Windows session-end
/// handler, and making it wait out its whole timeout only risks the system killing us
/// mid-shutdown.
fn write_events(writer: &mut MoonwatcherWriter, config: Option<&Config>, done: Option<Ack>) {
    match config {
        Some(config) => match writer.write(config) {
            Ok(_) => info!("Wrote successfully"),
            Err(e) => error!("Failed to write events, data will be lost!! Error: {e:?}"),
        }
        // Without a configuration nothing was ever sampled, so there is nothing buffered.
        None => debug!("Nothing to write, no configuration is loaded"),
    }

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

    /// A [`Desktop`] whose every outcome can be dictated, so the routine-versus-malfunction
    /// classification can be tested without a real desktop on either platform.
    #[derive(Default)]
    struct FakeDesktop {
        locked: bool,
        /// `Ok(None)` is nothing focused, `Err` is a broken implementation.
        active_window: Option<FakeWindow>,
        window_lookup_fails: bool,
        idle_fails: bool,
    }

    #[derive(Clone, Default)]
    struct FakeWindow {
        title: String,
        process_path: Option<PathBuf>,
    }

    impl Desktop for FakeDesktop {
        fn implementation_name(&self) -> &'static str { "FakeDesktop" }

        fn is_screen_locked(&self) -> bool { self.locked }

        fn get_idle_duration(&self) -> Result<Duration> {
            if self.idle_fails {
                bail!("xprintidle failed (exit status: 1): Can't open display");
            }
            Ok(Duration::from_secs(5))
        }

        fn get_active_window(&self) -> Result<Option<Box<dyn moonwatch_rs::watcher::core::Window>>> {
            if self.window_lookup_fails {
                bail!("xdotool failed (exit status: 1): Can't open display");
            }
            Ok(self.active_window.clone()
                .map(|window| Box::new(window) as Box<dyn moonwatch_rs::watcher::core::Window>))
        }
    }

    impl moonwatch_rs::watcher::core::Window for FakeWindow {
        fn get_title(&self) -> Result<String> { Ok(self.title.clone()) }

        fn get_process_id(&self) -> Result<u64> { Ok(1234) }

        fn get_process_path(&self) -> Result<PathBuf> {
            self.process_path.clone().context("Access is denied. (0x80070005)")
        }
    }

    fn sample(desktop: &FakeDesktop) -> Result<ActiveWindowEventResult> {
        get_window_event(desktop, Duration::from_secs(15))
    }

    #[test]
    fn nothing_focused_is_routine_and_not_reported() {
        let result = sample(&FakeDesktop { active_window: None, ..Default::default() });

        assert!(matches!(result, Ok(ActiveWindowEventResult::NoActiveWindow)), "got {result:?}");
        assert_eq!(sampling_problem(&result), None, "the tray must stay calm");
    }

    #[test]
    fn a_locked_screen_is_routine_and_not_reported() {
        let result = sample(&FakeDesktop { locked: true, ..Default::default() });

        assert!(matches!(result, Ok(ActiveWindowEventResult::DesktopLocked)), "got {result:?}");
        assert_eq!(sampling_problem(&result), None);
    }

    /// The case this whole change exists for: xdotool (or its equivalent) stops working.
    #[test]
    fn a_broken_implementation_is_reported_to_the_tray() {
        let result = sample(&FakeDesktop { window_lookup_fails: true, ..Default::default() });

        let problem = sampling_problem(&result).expect("a malfunction must be reported");
        assert!(problem.contains("Can't open display"), "got {problem:?}");
    }

    /// Idle time is part of gathering an event, so failing to read it is a malfunction too -
    /// it used to be reported as zero idle time, ie. silently wrong data.
    #[test]
    fn failing_to_read_idle_time_is_reported() {
        let result = sample(&FakeDesktop {
            active_window: Some(FakeWindow::default()),
            idle_fails: true,
            ..Default::default()
        });

        let problem = sampling_problem(&result).expect("a malfunction must be reported");
        assert!(problem.contains("xprintidle"), "got {problem:?}");
    }

    /// An elevated window on Windows: the path cannot be read, but the sample is still good.
    #[test]
    fn an_unreadable_process_path_still_records_the_event() {
        let result = sample(&FakeDesktop {
            active_window: Some(FakeWindow {
                title: "Task Manager".to_string(),
                process_path: None,
            }),
            ..Default::default()
        });

        assert_eq!(sampling_problem(&result), None, "this is not a malfunction");
        let Ok(ActiveWindowEventResult::Window { e }) = result else {
            panic!("the event should still be recorded");
        };
        assert_eq!(e.process_path, None);
        assert!(e.to_json()["data"]["processPath"].is_null());
    }

    #[test]
    fn a_normal_sample_produces_an_event_with_its_path() {
        let result = sample(&FakeDesktop {
            active_window: Some(FakeWindow {
                title: "Some Window".to_string(),
                process_path: Some(PathBuf::from("/usr/bin/firefox")),
            }),
            ..Default::default()
        });

        assert_eq!(sampling_problem(&result), None);
        let Ok(ActiveWindowEventResult::Window { e }) = result else {
            panic!("expected an event, got {result:?}");
        };
        assert_eq!(e.process_path.as_deref(), Some(Path::new("/usr/bin/firefox")));
        assert_eq!(e.idle_for, Duration::from_secs(5));
    }

    #[test]
    fn test_writer_writes_expected_line_to_temp_dir() {
        // Unique temp dir that does not exist yet, so write() also exercises
        // its output-directory creation branch.
        let output_dir = std::env::temp_dir().join(format!("moonwatch-test-{}", Uuid::now_v7()));
        assert!(!output_dir.exists());

        let config = Config {
            output_dir: output_dir.clone(),
            sample_every: Duration::from_secs(15),
            write_every: Duration::from_secs(60),
            tags: vec![],
            ignore: vec![],
            anonymize: vec![],
        };

        let event = ActiveWindowEventV1::new(
            Duration::from_secs(5),
            "Test Window".to_string(),
            Some(PathBuf::from("/path/to/app")),
            Duration::from_secs(1),
        );
        // Capture the expected serialization before pushing: write() drains the
        // buffer and ActiveWindowEvent is not Clone.
        let expected_line = event.to_json().to_string();

        let mut writer = MoonwatcherWriter::new();
        writer.push(event);
        writer.write(&config).expect("write() should succeed");

        // Exactly one .jsonl file should have been produced.
        let jsonl_files: Vec<PathBuf> = fs::read_dir(&output_dir)
            .expect("output dir should exist after write()")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        assert_eq!(jsonl_files.len(), 1, "expected exactly one .jsonl file");

        let contents = fs::read_to_string(&jsonl_files[0]).expect("read output file");
        assert_eq!(contents, format!("{expected_line}\n"));
        assert!(contents.contains(r#""type":"ActiveWindowEventV1""#));
        assert!(contents.contains("/path/to/app"));

        // Clean up.
        fs::remove_dir_all(&output_dir).ok();
    }

    /// A `WriteNow`/`Terminate` request must always be acknowledged, so that a waiting
    /// session-end handler is released instead of sitting out its timeout.
    #[test]
    fn write_events_acknowledges_even_with_nothing_to_write() {
        let config = Config {
            output_dir: std::env::temp_dir().join(format!("moonwatch-test-{}", Uuid::now_v7())),
            sample_every: Duration::from_secs(15),
            write_every: Duration::from_secs(60),
            tags: vec![],
            ignore: vec![],
            anonymize: vec![],
        };

        let mut writer = MoonwatcherWriter::new();
        let (ack_sender, ack_receiver) = crossbeam_channel::bounded(1);
        write_events(&mut writer, Some(&config), Some(ack_sender));

        assert_eq!(ack_receiver.try_recv(), Ok(()));
        assert!(!config.output_dir.exists(), "an empty write should not create the output dir");
    }

    /// Shutting down while config.json is broken must not hang the session-end handler.
    #[test]
    fn write_events_acknowledges_when_no_configuration_is_loaded() {
        let mut writer = MoonwatcherWriter::new();
        let (ack_sender, ack_receiver) = crossbeam_channel::bounded(1);
        write_events(&mut writer, None, Some(ack_sender));

        assert_eq!(ack_receiver.try_recv(), Ok(()));
    }

    /// A broken config.json must be a reportable problem rather than something that stops
    /// the daemon, and the message has to name the file and the syntax error so the user can
    /// act on what the tray shows them.
    #[test]
    fn load_active_reports_a_malformed_config_instead_of_panicking() {
        let dir = std::env::temp_dir().join(format!("moonwatch-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");
        fs::write(&config_path, r#"{ "main": { "output_dir": "log", }"#).expect("write config");

        // .err() rather than .expect_err(): Active holds a Box<dyn Desktop>, so it is not Debug.
        let error = load_active(&config_path).err().expect("malformed JSON should not load");
        let message = format!("{error:#}");

        assert!(message.contains("config.json could not be loaded"), "got {message:?}");
        assert!(message.contains("line"), "should point at the syntax error: {message:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sampling_without_a_configuration_has_no_timers_and_nothing_to_write() {
        let sampling = Sampling::idle();

        assert!(sampling.config().is_none());
        assert!(sampling.sample_every().is_none());
        // `never()` receivers, so select! just waits for signals.
        assert!(sampling.sample_tick.try_recv().is_err());
        assert!(sampling.writer_tick.try_recv().is_err());
    }

    /// End-to-end tests of the worker and the status it publishes.
    ///
    /// Windows only: the unix `Desktop` implementation shells out to `xdotool` and
    /// `xprintidle` against a live X11 session, which CI does not have, so anything here
    /// that expects sampling to actually start would fail there for unrelated reasons.
    #[cfg(windows)]
    mod worker {
        use super::*;
        use moonwatch_rs::watcher::status::{MoonwatcherStatus, StatusIcon};

        struct Fixture {
            dir: PathBuf,
            config_path: PathBuf,
            signals: crossbeam_channel::Sender<MoonwatcherSignal>,
            status: SharedStatus,
            output_dir: SharedOutputDir,
            worker: Option<thread::JoinHandle<Result<()>>>,
        }

        const VALID_CONFIG: &str = r#"{
            "main": { "output_dir": "log", "sample_every_sec": 1, "write_every_sec": 3600 }
        }"#;
        const BROKEN_CONFIG: &str = r#"{ "main": { "output_dir": "log", }"#;

        impl Fixture {
            fn start(initial_config: &str) -> Fixture {
                let dir = std::env::temp_dir().join(format!("moonwatch-test-{}", Uuid::now_v7()));
                fs::create_dir_all(&dir).expect("create temp dir");
                let config_path = dir.join("config.json");
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

        /// The flow this feature exists for: the user breaks config.json, sees the problem in
        /// the tray, fixes it and reloads.
        #[test]
        fn a_failed_reload_is_reported_while_recording_continues() {
            let fixture = Fixture::start(VALID_CONFIG);

            let healthy = fixture.wait_for("recording to start", |s| {
                s.recording == RecordingState::Recording
            });
            assert_eq!(healthy.icon(), StatusIcon::Recording);
            assert_eq!(healthy.menu_line(), "Recording every 1 s");
            assert!(fixture.log_folder().is_some(), "Open log folder should be usable");

            fixture.write_config(BROKEN_CONFIG);
            fixture.send(MoonwatcherSignal::ReloadConfig);

            let broken = fixture.wait_for("the bad config to be reported",
                                          |s| s.config_problem.is_some());
            assert_eq!(broken.icon(), StatusIcon::Problem, "the error icon should win");
            assert_eq!(broken.recording, RecordingState::Recording,
                       "the previous configuration should still be recording");
            let line = broken.menu_line();
            assert!(line.starts_with("config.json could not be loaded"), "got {line:?}");
            assert!(line.ends_with("previous settings still in use"), "got {line:?}");

            fixture.write_config(VALID_CONFIG);
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
            let fixture = Fixture::start(BROKEN_CONFIG);

            let stopped = fixture.wait_for("the startup failure to be reported", |s| {
                s.config_problem.is_some()
            });
            assert_eq!(stopped.recording, RecordingState::Stopped);
            assert_eq!(stopped.icon(), StatusIcon::Problem);
            assert!(stopped.menu_line().starts_with("Not recording - config.json could not be loaded"),
                    "got {:?}", stopped.menu_line());
            assert!(stopped.sample_every.is_none());
            assert!(fixture.log_folder().is_none(),
                    "Open log folder should be disabled until a config loads");

            fixture.write_config(VALID_CONFIG);
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
