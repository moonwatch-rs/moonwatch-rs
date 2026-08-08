use std::fs::File;
use std::path::{Path, PathBuf};
use crate::core::model::config::{Config, MainConfig};
use anyhow::{Context, Result};
use crate::pipeline::model::config::PipelineConfig;
use crate::recorder::model::config::RecorderConfig;

pub fn read_config(main_config_path: &Path) -> Result<Config> {
    let main_config_file = File::open(main_config_path)?;
    let main_config: MainConfig = serde_json::from_reader(main_config_file)?;

    let recorder_config: RecorderConfig = match &main_config.recorder_config_path {
        None => RecorderConfig::new(),
        Some(path) => {
            let recorder_config_path = main_config_path
                .parent()
                .context("cannot get parent of recorder_config_path")?
                .join(path);
            read_recorder_config(recorder_config_path.as_path())?
        }
    };

    let pipeline_config: PipelineConfig = match &main_config.pipeline_config_path {
        None => PipelineConfig::new(),
        Some(path) => {
            let pipeline_config_path = main_config_path
                .parent()
                .context("cannot get parent of pipeline_config_path")?
                .join(path);
            read_pipeline_config(pipeline_config_path.as_path())?
        }
    };

    let log_directory: PathBuf = (&main_config.log_directory).into();
    let log_output_subdirectory: PathBuf = [
        &main_config.log_directory,
        &main_config.log_output_subdirectory
    ].iter().collect();

    Ok(Config {
        main_config,
        recorder_config,
        pipeline_config,
        log_directory: log_directory.canonicalize()?,
        log_output_subdirectory: log_output_subdirectory.canonicalize()?,
    })
}

pub fn read_recorder_config(path: &Path) -> Result<RecorderConfig> {
    let file = File::open(path)?;
    let config: RecorderConfig = serde_json::from_reader(file)?;
    Ok(config)
}

pub fn read_pipeline_config(path: &Path) -> Result<PipelineConfig> {
    let file = File::open(path)?;
    let config: PipelineConfig = serde_json::from_reader(file)?;
    Ok(config)
}
