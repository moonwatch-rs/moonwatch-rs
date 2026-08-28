//! This module contains Moonwatch ETL pipeline for batch processing of captured logs.

pub mod model;
pub mod parser;
pub mod transform;
pub mod pipeline;

use std::path::Path;

use anyhow::{Context, Result};
use log::info;

use crate::core::model::config::Config;
use crate::pipeline::pipeline::MoonwatchPipeline;

/// Read the configuration and run the ETL pipeline over the recorded logs once.
///
/// Shared by the `pipeline` subcommand and by the daemon's "Run data pipeline" tray action,
/// so that both read the same configuration and write the same files. The configuration is
/// read here rather than passed in: the tray action runs on a thread of its own, while the
/// daemon's own copy of the configuration belongs to the worker thread.
///
/// A configuration that cannot be read is an error rather than something to work around -
/// there is nothing useful to do without it, and both callers have somewhere to report it.
pub fn run_pipeline(config_path: &Path) -> Result<()> {
    let config = Config::from_file(config_path)
        .with_context(|| format!("could not read {}", config_path.display()))?;

    info!("Reading logs from {}", config.log_directory.display());
    info!("Writing {:?} output to {}",
          config.main_config.pipeline_output_format,
          config.pipeline_output_directory.display());

    MoonwatchPipeline::from_config(config).write().context("the pipeline failed")?;

    info!("Pipeline finished");
    Ok(())
}
