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
    fn is_screen_locked(&self) -> bool;
    fn get_idle_duration(&self) -> Duration;
    fn get_active_window(&self) -> Result<Box<dyn Window>>;
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
    pub process_path: PathBuf,
    pub tags: LinkedList<String>,
    pub _anonymize: bool,
}

impl ActiveWindowEventV1 {
    pub fn new(idle_for: Duration,
               window_title: String,
               process_path: PathBuf,
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

        let process_path = if self._anonymize {
            serde_json::Value::Null
        } else {
            serde_json::Value::from(self.process_path.to_str().unwrap_or(""))
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

#[derive(Debug)]
pub enum MoonwatcherSignal {
    ReloadConfig,
    Terminate
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
            PathBuf::from("/path/to/app"),
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
}
