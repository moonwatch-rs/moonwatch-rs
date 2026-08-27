use std::fs::File;
use std::path::{absolute, Path, PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use anyhow::{Context, Result};
use crate::core::common::config_dir;
use crate::pipeline::model::config::PipelineTransformConfig;
use crate::recorder::model::config::RecorderConfig;


/// Name of the main configuration file inside a Moonwatch.rs directory.
pub const MAIN_CONFIG_FILE_NAME: &str = "main_config.json";

pub fn default_main_config() -> String {
    format!("./{MAIN_CONFIG_FILE_NAME}")
}

pub(crate) fn default_main_config_schema() -> String {
    "./schemas/main_config.schema.json".to_string()
}

/// This is the entrypoint to Moonwatch configuration.
/// It describes how events are logged and stored on this particular machine.
/// This top-level configuration should not be shared among different machines.
/// More detailed configuration of event gathering and post-hoc analytics
/// lives in separate linked configuration files - it is useful to share
/// these if you have Moonwatch on multiple machines.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MainConfig {
    #[serde(rename = "$schema", default = "default_main_config_schema")]
    #[schemars(skip)]
    pub schema: String,
    
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

    /// # Pipeline output directory
    /// Moonwatch defines an ETL-like pipeline for further data analysis, as defined by
    /// `pipeline_config_path`. This is the directory where the resulting flat files
    /// will be stored. Relative path is interpreted as relative to the directory
    /// with this configuration file.
    #[schemars(example = &"./output")]
    pub pipeline_output_directory: String,

    /// # Pipeline output format
    /// Moonwatch defines an ETL-like pipeline for further data analysis, as defined by
    /// `pipeline_config_path`. This is the format of the resulting flat files.
    pub pipeline_output_format: PipelineOutputFormat,
}

impl MainConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let config: Self = serde_json::from_reader(file)?;
        Ok(config)
    }
}

#[derive(Debug)]
pub struct Config {
    pub main_config: MainConfig,
    pub recorder_config: RecorderConfig,
    pub pipeline_config: PipelineTransformConfig,

    /// Absolute path to directory with all Moonwatch logs.
    pub log_directory: PathBuf,

    /// Absolute path to directory with where this instance of Moonwatch should write its logs.
    pub log_output_subdirectory: PathBuf,

    /// Absolute path to directory where Moonwatch should dump results of its ETL pipeline.
    pub pipeline_output_directory: PathBuf,
}

impl Config {
    pub fn from_file(main_config_path: impl AsRef<Path>) -> Result<Self> {
        let main_config_path = main_config_path.as_ref();
        let main_config = MainConfig::from_file(main_config_path)?;

        // Every relative path in `MainConfig` is relative to the directory holding it.
        let config_dir = config_dir(main_config_path);

        let recorder_config: RecorderConfig = match &main_config.recorder_config_path {
            None => RecorderConfig::new(),
            Some(path) => {
                let recorder_config_path = config_dir.join(path);
                RecorderConfig::from_file(&recorder_config_path)
                    .with_context(|| format!("could not read recorder config {}",
                                             recorder_config_path.display()))?
            }
        };

        let pipeline_config: PipelineTransformConfig = match &main_config.pipeline_config_path {
            None => PipelineTransformConfig::new(),
            Some(path) => {
                let pipeline_config_path = config_dir.join(path);
                PipelineTransformConfig::from_file(&pipeline_config_path)
                    .with_context(|| format!("could not read pipeline config {}",
                                             pipeline_config_path.display()))?
            }
        };

        // `absolute` rather than `canonicalize`: these directories are created on demand,
        // so they need not exist yet at the time the configuration is read.
        let log_directory: PathBuf = absolute(config_dir.join(&main_config.log_directory))?;
        let log_output_subdirectory: PathBuf =
            absolute(log_directory.join(&main_config.log_output_subdirectory))?;
        let pipeline_output_directory: PathBuf =
            absolute(config_dir.join(&main_config.pipeline_output_directory))?;

        Ok(Self {
            main_config,
            recorder_config,
            pipeline_config,
            log_directory,
            log_output_subdirectory,
            pipeline_output_directory,
        })
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PipelineOutputFormat {
    Parquet,
    Csv,
}

impl PipelineOutputFormat {
    pub fn get_file_extension(&self) -> &str {
        match self {
            PipelineOutputFormat::Parquet => ".parquet",
            PipelineOutputFormat::Csv => ".csv",
        }
    }
}
