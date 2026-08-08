use std::path::PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::pipeline::model::config::PipelineConfig;
use crate::recorder::model::config::RecorderConfig;

/// This is the entrypoint to Moonwatch configuration.
/// It describes how events are logged and stored on this particular machine.
/// This top-level configuration should not be shared among different machines.
/// More detailed configuration of event gathering and post-hoc analytics
/// lives in separate linked configuration files - it is useful to share
/// these if you have Moonwatch on multiple machines.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MainConfig {
    /// # Log directory
    /// Path to directory where all Moonwatch logs are stored.
    /// This will typically point to some kind of synchronized directory across devices.
    /// Relative path is interpreted as relative to the directory with this configuration file.
    #[schemars(example = &"./logs")]
    pub log_directory: String,

    /// # Log subdirectory for this instance
    /// Subdirectory of `logDirectory` where logs gathered from this computer will be stored.
    /// This is optional - you can use '.' to just put all logs into one directory.
    pub log_output_subdirectory: String,

    /// # Event sampling period (in seconds)
    /// This parameter defines how often the active window is queried, producing a `ActiveWindowEvent`
    /// entry in the output log.
    pub sample_every_sec: i32,

    /// # Log write period (in seconds)
    /// The background service gathers events in memory and in regular intervals writes the current
    /// buffer to a new file, clearing the memory buffer. This should be relatively infrequent,
    /// as to prevent creating needlessly many files. Important note - there are problems with
    /// graceful shutdown on Windows, so the last run may be lost. Due to this problem, the default
    /// write period is much shorter than on Linux.
    pub write_every_sec: i32,

    /// # Path to recorder config
    /// This configuration file describes how the Moonwatch service processes events before
    /// they are written to the log (tagging, redacting). See definition of `RecordConfig`.
    /// Relative path is interpreted as relative to the directory with this configuration file;
    /// it can be useful to put this into a synchronized directory so that all instances can use the same
    /// configuration.
    #[schemars(example = &"./recorder.json")]
    pub recorder_config_path: Option<String>,

    /// # Path to pipeline config
    /// This configuration file describes how the log data is ingested for analysis
    /// (removing intervals without user interaction, categorizing, etc.). See definition
    /// of `PipelineConfig`.
    /// Relative path is interpreted as relative to the directory with this configuration file;
    /// it can be useful to put this into a synchronized directory so that all instances can use the same
    /// configuration.
    #[schemars(example = &"./pipeline.json")]
    pub pipeline_config_path: Option<String>,
}

#[derive(Debug)]
pub struct Config {
    pub main_config: MainConfig,
    pub recorder_config: RecorderConfig,
    pub pipeline_config: PipelineConfig,

    /// Absolute path to directory with all Moonwatch logs.
    pub log_directory: PathBuf,

    /// Absolute path to directory with where this instance of Moonwatch should write its logs.
    pub log_output_subdirectory: PathBuf,
}
