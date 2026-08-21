use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use chrono::TimeDelta;
use crate::core::model::config::{default_main_config_schema, MainConfig, default_main_config, PipelineOutputFormat};
use crate::pipeline::model::config::{default_pipeline_config_schema, PipelineTransformConfig, default_pipeline_config};
use crate::recorder::model::config::{default_recorder_config_schema, RecorderConfig, default_recorder_config};

/// This struct is responsible for writing the default config files and their JSON schemas.
pub struct ConfigWriter {
    pub moonwatch_dir: PathBuf,
}

impl ConfigWriter {
    pub fn new(moonwatch_dir: impl AsRef<Path>) -> Self {
        Self {
            moonwatch_dir: moonwatch_dir.as_ref().to_path_buf(),
        }
    }

    /// Write default configuration files. In case the files exist, it is advisable
    /// to keep them and do not overwrite - to allow the user to migrate them
    /// as necessary to the latest schema.
    pub fn write_default_configs(&self, overwrite_existing: bool) -> Result<()> {
        let path = self.moonwatch_dir.join(default_main_config());
        if !path.exists() || overwrite_existing {
            let main_config = MainConfig {
                schema: default_main_config_schema().into(),
                log_directory: "./logs".into(),
                log_output_subdirectory: ".".into(),
                sample_every_sec: 15,
                write_every_sec: 3600,
                recorder_config_path: Some(default_recorder_config()),
                pipeline_config_path: Some(default_pipeline_config()),
                pipeline_output_directory: "./output".into(),
                pipeline_output_format: PipelineOutputFormat::Parquet,
            };
            let json = serde_json::to_string_pretty(&main_config)
                .expect("failed to serialize MainConfig");
            fs::write(&path, json)
                .with_context(|| format!("failed to write {}", &path.display()))?;
        }

        let path = self.moonwatch_dir.join(default_recorder_config());
        if !path.exists() || overwrite_existing {
            let recorder_config = RecorderConfig {
                schema: default_recorder_config_schema().into(),
                active_window_event_rules: vec![],
            };
            let json = serde_json::to_string_pretty(&recorder_config)
                .expect("failed to serialize RecorderConfig");
            fs::write(&path, json)
                .with_context(|| format!("failed to write {}", &path.display()))?;
        }

        let path = self.moonwatch_dir.join(default_pipeline_config());
        if !path.exists() || overwrite_existing {
            let pipeline_config = PipelineTransformConfig {
                schema: default_pipeline_config_schema().into(),
                active_event_rules: vec![],
                active_event_max_duration: TimeDelta::minutes(10),
            };
            let json = serde_json::to_string_pretty(&pipeline_config)
                .expect("failed to serialize PipelineTransformConfig");
            fs::write(&path, json)
                .with_context(|| format!("failed to write {}", &path.display()))?;
        }

        Ok(())
    }

    /// Write JSON schemas into the `schemas` subdirectory. Overwrite any previous
    /// schemas so that any config files pick up the new schemas and highlight any
    /// places where the config files need user attention and fixing.
    pub fn write_schemas(&self) -> Result<()> {
        let path = self.moonwatch_dir.join("schemas");
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", &path.display()))?;

        let path = self.moonwatch_dir.join(default_main_config_schema());
        let schema = schemars::schema_for!(MainConfig);
        let json = serde_json::to_string_pretty(&schema)
            .expect("failed to serialize MainConfig schema");
        fs::write(&path, json)
            .with_context(|| format!("failed to write {}", &path.display()))?;

        let path = self.moonwatch_dir.join(default_recorder_config_schema());
        let schema = schemars::schema_for!(RecorderConfig);
        let json = serde_json::to_string_pretty(&schema)
            .expect("failed to serialize RecorderConfig schema");
        fs::write(&path, json)
            .with_context(|| format!("failed to write {}", &path.display()))?;

        let path = self.moonwatch_dir.join(default_pipeline_config_schema());
        let schema = schemars::schema_for!(PipelineTransformConfig);
        let json = serde_json::to_string_pretty(&schema)
            .expect("failed to serialize PipelineTransformConfig schema");
        fs::write(&path, json)
            .with_context(|| format!("failed to write {}", &path.display()))?;

        Ok(())
    }
}
