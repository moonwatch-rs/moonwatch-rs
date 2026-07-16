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
pub struct ActiveWindowEvent {
    pub time: DateTime::<Utc>,
    pub duration: Duration,
    pub hostname: String,
    pub username: String,
    pub idle_for: Duration,
    pub window_title: String,
    pub process_path: PathBuf,
    pub tags: LinkedList<String>,
    pub anonymize: bool,
}

impl ActiveWindowEvent {
    pub fn new(idle_for: Duration,
               window_title: String,
               process_path: PathBuf,
               duration: Duration) -> ActiveWindowEvent {
        ActiveWindowEvent {
            time: Utc::now(),
            duration,
            hostname: whoami::hostname().unwrap_or_default(),
            username: whoami::username().unwrap_or_default(),
            idle_for,
            window_title,
            process_path,
            tags: LinkedList::new(),
            anonymize: false,
        }
    }

    pub fn to_json(&self) -> json::JsonValue {
        let tags: Vec<String> = self.tags.iter().map(|x| String::from(x)).collect();

        if self.anonymize {
            json::object! {
                "type": "ActiveWindowEvent",
                "time": self.time.to_rfc3339(),
                "duration": self.duration.as_secs_f32().round(),
                "hostname": self.hostname.as_str(),
                "username": self.username.as_str(),
                "idle_for": self.idle_for.as_secs_f32().round(),
                "process_path": json::Null,
                "tags": tags,
            }
        } else {
            json::object! {
                "type": "ActiveWindowEvent",
                "time": self.time.to_rfc3339(),
                "duration": self.duration.as_secs_f32().round(),
                "hostname": self.hostname.as_str(),
                "username": self.username.as_str(),
                "idle_for": self.idle_for.as_secs_f32().round(),
                "process_path": self.process_path.to_str().unwrap_or(""),
                "tags": tags,
            }
        }
    }
}

#[derive(Debug)]
pub enum MoonwatcherSignal {
    ReloadConfig,
    Terminate
}
