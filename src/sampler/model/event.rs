use std::path::{Path, PathBuf};
use std::time::Duration;
use chrono::{DateTime, TimeDelta, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::core::model::event::Event;
use crate::core::model::event::ActiveWindowEventV1Data;

/// Events logged by Moonwatch (runtime representation)
#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeEvent {
    ActiveWindowEvent(RuntimeActiveWindowEvent),
}

/// A runtime version of ActiveWindowEventV1
#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeActiveWindowEvent {
    pub time: DateTime<Utc>,
    pub data: ActiveWindowEventV1Data,
    pub window_title: String,
}

#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeActiveWindowEventStringAttribute {
    WindowTitle,
    ProcessPath,
    ProcessName,
    Hostname,
    Username,
}

impl RuntimeActiveWindowEvent {
    /// Build an event for a window that was found to be active right now.
    ///
    /// `duration` is how long the window is assumed to have been active - Moonwatch samples
    /// at regular intervals and credits the whole interval to whatever it caught.
    ///
    /// `process_path` is `None` when it could not be determined: the process may be
    /// elevated, owned by another user, or already gone. The event is still worth recording,
    /// so this is not a reason to drop the sample. Tagging and redaction are not applied
    /// here - that is the recorder's job, see `RecorderConfig`.
    pub fn new(idle_for: Duration,
               window_title: String,
               process_path: Option<PathBuf>,
               duration: Duration) -> Self {
        RuntimeActiveWindowEvent {
            time: Utc::now(),
            data: ActiveWindowEventV1Data {
                duration: whole_seconds(duration),
                hostname: whoami::hostname().unwrap_or_default(),
                username: whoami::username().unwrap_or_default(),
                idle_for: whole_seconds(idle_for),
                process_path: process_path
                    .map(|path| path.to_string_lossy().into_owned()),
                tags: vec![],
            },
            window_title,
        }
    }

    pub fn extract_string_attribute(self: &Self, attribute: &RuntimeActiveWindowEventStringAttribute) -> Option<String> {
        match attribute {
            RuntimeActiveWindowEventStringAttribute::WindowTitle => Some(self.window_title.clone()),
            RuntimeActiveWindowEventStringAttribute::ProcessPath => self.data.process_path.clone(),
            RuntimeActiveWindowEventStringAttribute::ProcessName => self.get_process_name(),
            RuntimeActiveWindowEventStringAttribute::Hostname => Some(self.data.hostname.clone()),
            RuntimeActiveWindowEventStringAttribute::Username => Some(self.data.username.clone()),
        }
    }

    pub fn get_process_name(self: &Self) -> Option<String> {
        Path::new(&self.data.process_path.as_deref()?).file_stem()?.to_str().map(String::from)
    }
}

impl Into<Event> for RuntimeEvent {
    fn into(self) -> Event {
        match self {
            RuntimeEvent::ActiveWindowEvent(e) => {
                Event::ActiveWindowEventV1 {
                    time: e.time,
                    data: ActiveWindowEventV1Data {
                        ..e.data
                    },
                }
            }
        }
    }
}

impl Into<RuntimeEvent> for RuntimeActiveWindowEvent {
    fn into(self) -> RuntimeEvent {
        RuntimeEvent::ActiveWindowEvent(self)
    }
}

/// Durations are logged as whole seconds, so round on the way in rather than letting
/// serialization decide.
fn whole_seconds(duration: Duration) -> TimeDelta {
    TimeDelta::seconds(duration.as_secs_f64().round() as i64)
}
