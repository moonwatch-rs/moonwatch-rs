use std::path::Path;
use chrono::{DateTime, Utc};
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
