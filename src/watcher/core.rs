use std::collections::LinkedList;
use std::path::PathBuf;
use std::time::Duration;
use chrono::{DateTime, Utc};
use anyhow::Result;

pub trait Window {
    fn get_title(&self) -> Result<String>;
    fn get_process_id(&self) -> Result<u64>;
    fn get_process_path(&self) -> Result<PathBuf>;
}

pub trait Desktop {
    fn implementation_name(&self) -> &'static str;
    fn check_implementation_available(&self) -> Result<()> {
        Ok(())
    }

    /// Whether the screen is locked. Implementations degrade to `false` rather than
    /// reporting an error, because a missing or idle screensaver is not a malfunction.
    fn is_screen_locked(&self) -> bool;

    /// How long the user has been idle.
    ///
    /// `Err` means the implementation could not find out. That is never routine, so it is
    /// reported to the user - unlike the previous infallible version, which reported zero
    /// idle time (ie. "the user is active") whenever it failed.
    fn get_idle_duration(&self) -> Result<Duration>;

    /// The focused window.
    ///
    /// `Ok(None)` means nothing is focused, which is routine: it happens whenever focus is on
    /// the desktop, in between window switches, and on the lock screen. `Err` is reserved for
    /// the implementation itself not working - a missing tool, an unreachable display server,
    /// output that could not be understood - and is what turns the tray icon red.
    fn get_active_window(&self) -> Result<Option<Box<dyn Window>>>;

    fn before_main_loop_start(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct ActiveWindowEventV1 {
    pub time: DateTime::<Utc>,
    pub duration: Duration,
    pub hostname: String,
    pub username: String,
    pub idle_for: Duration,
    pub window_title: String,
    /// Absent when it could not be determined: the process may be elevated, owned by another
    /// user, or already gone. The event is still worth recording, so this is `None` rather
    /// than a reason to drop the sample.
    pub process_path: Option<PathBuf>,
    pub tags: LinkedList<String>,
    pub _anonymize: bool,
}

impl ActiveWindowEventV1 {
    pub fn new(idle_for: Duration,
               window_title: String,
               process_path: Option<PathBuf>,
               duration: Duration) -> ActiveWindowEventV1 {
        ActiveWindowEventV1 {
            time: Utc::now(),
            duration,
            hostname: whoami::hostname().unwrap_or_default(),
            username: whoami::username().unwrap_or_default(),
            idle_for,
            window_title,
            process_path,
            tags: LinkedList::new(),
            _anonymize: false,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let tags: Vec<String> = self.tags.iter().cloned().collect();

        let process_path = match &self.process_path {
            Some(path) if !self._anonymize => {
                serde_json::Value::from(path.to_str().unwrap_or(""))
            }
            // Redacted, or never determined in the first place.
            _ => serde_json::Value::Null,
        };

        serde_json::json!({
            "type": "ActiveWindowEventV1",
            "time": self.time.to_rfc3339(),
            "data": {
                "duration": self.duration.as_secs_f32().round(),
                "hostname": self.hostname,
                "username": self.username,
                "idleFor": self.idle_for.as_secs_f32().round(),
                "processPath": process_path,
                "tags": tags,
            }
        })
    }
}

/// One-shot channel handed to the worker along with a request, so that the requester can
/// wait until the request has actually been carried out.
pub type Ack = crossbeam_channel::Sender<()>;

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
/// messages) and by the OS signal handlers; none of them own the `MoonwatcherWriter`.
/// `terminate_and_wait` is what makes a clean logout possible: it asks the worker to
/// flush and blocks until it confirms, so we do not return from `WM_QUERYENDSESSION`
/// before the data is on disk.
#[derive(Clone)]
pub struct WorkerHandle {
    signals: crossbeam_channel::Sender<MoonwatcherSignal>,
}

impl WorkerHandle {
    pub fn new(signals: crossbeam_channel::Sender<MoonwatcherSignal>) -> WorkerHandle {
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
                log::error!("Worker thread did not respond within {timeout:?}, data may be lost");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time JSON Schema (draft 2020-12) validator, built from the
    /// checked-in schema. Used only in tests to assert that serialized events
    /// conform to `src/schema/events/ActiveWindowEventV1.json`.
    #[jsonschema::validator(path = "src/schema/events/ActiveWindowEventV1.json", draft = Draft202012)]
    struct ActiveWindowEventSchema;

    fn sample_event() -> ActiveWindowEventV1 {
        ActiveWindowEventV1::new(
            Duration::from_secs(5),
            "Test Window".to_string(),
            Some(PathBuf::from("/path/to/app")),
            Duration::from_secs(15),
        )
    }

    #[test]
    fn to_json_without_anonymize_keeps_process_path_and_matches_schema() {
        let event = sample_event();
        assert!(!event._anonymize);
        let value = event.to_json();

        assert_eq!(value["type"].as_str(), Some("ActiveWindowEventV1"));
        assert_eq!(value["data"]["processPath"].as_str(), Some("/path/to/app"));
        assert!(value["time"].is_string());
        assert!(value["data"]["duration"].is_number());
        assert!(value["data"]["idleFor"].is_number());
        assert_eq!(value["data"]["tags"], serde_json::json!([]));

        ActiveWindowEventSchema::validate(&value)
            .expect("serialized event should be valid against the schema");
    }

    #[test]
    fn to_json_with_anonymize_nulls_process_path_and_matches_schema() {
        let mut event = sample_event();
        event._anonymize = true;
        let value = event.to_json();

        assert_eq!(value["type"].as_str(), Some("ActiveWindowEventV1"));
        assert!(
            value["data"]["processPath"].is_null(),
            "processPath should be null when anonymized, got {:?}",
            value["data"]["processPath"]
        );
        // The remaining fields are still present and unredacted.
        assert!(value["data"]["hostname"].is_string());
        assert!(value["data"]["username"].is_string());

        ActiveWindowEventSchema::validate(&value)
            .expect("anonymized event should be valid against the schema");
    }

    /// A window whose process could not be opened (elevated, or owned by another user) is
    /// still recorded, just without a path.
    #[test]
    fn to_json_with_unknown_process_path_nulls_it_and_matches_schema() {
        let event = ActiveWindowEventV1::new(
            Duration::from_secs(5),
            "Task Manager".to_string(),
            None,
            Duration::from_secs(15),
        );
        let value = event.to_json();

        assert!(value["data"]["processPath"].is_null(),
                "processPath should be null when unknown, got {:?}", value["data"]["processPath"]);
        assert!(value["data"]["duration"].is_number(), "the event is still usable for totals");

        ActiveWindowEventSchema::validate(&value)
            .expect("event with an unknown path should be valid against the schema");
    }
}
