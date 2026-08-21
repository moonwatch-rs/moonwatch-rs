use anyhow::Result;
use polars::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::core::model::config::PipelineOutputFormat;
use crate::core::model::config::Config;
use crate::pipeline::model::config::PipelineTransformConfig;
use crate::pipeline::parser::MoonwatchLogParser;
use crate::pipeline::transform::evaluate_active_window_rules;

/// This struct represents Moonwatch ETL pipeline
pub struct MoonwatchPipeline {
    pub config: PipelineTransformConfig,
    pub parser: MoonwatchLogParser,
    pub pipeline_output_directory: PathBuf,
    pub pipeline_output_format: PipelineOutputFormat,
}

impl MoonwatchPipeline {
    pub fn from_config(config: Config) -> Self {
        Self {
            config: config.pipeline_config,
            parser: MoonwatchLogParser::new(vec![
                config.log_directory.join("**/*.jsonl"),
                config.log_directory.join("**/*.jsonl.gz"),
            ]),
            pipeline_output_directory: config.pipeline_output_directory,
            pipeline_output_format: config.main_config.pipeline_output_format,
        }
    }

    pub fn get_active_event_lf(&self) -> LazyFrame {
        let lf = self.parser.get_input_lf().unwrap();
        let active_event_lf = MoonwatchLogParser::get_active_event_lf(lf);
        evaluate_active_window_rules(active_event_lf, &self.config.active_event_rules)
    }

    pub fn get_unlock_event_lf(&self) -> LazyFrame {
        let lf = self.parser.get_input_lf().unwrap();
        MoonwatchLogParser::get_unlock_event_lf(lf)
    }

    pub fn write(&self) -> Result<()> {
        let ext = self.pipeline_output_format.get_file_extension();
        Self::write_data_file(
            self.pipeline_output_directory.join(format!("active_events{}", ext)),
            self.get_active_event_lf(),
            self.pipeline_output_format,
        )?;
        Self::write_data_file(
            self.pipeline_output_directory.join(format!("unlock_events{}", ext)),
            self.get_unlock_event_lf(),
            self.pipeline_output_format,
        )?;
        Ok(())
    }

    fn write_data_file(path: impl AsRef<Path>, lf: LazyFrame, format: PipelineOutputFormat) -> Result<()> {
        let file_format = match format {
            PipelineOutputFormat::Parquet => {
                FileWriteFormat::Parquet(Arc::new(ParquetWriteOptions::default()))
            }
            PipelineOutputFormat::Csv => {
                FileWriteFormat::Csv(CsvWriterOptions::default())
            }
        };

        let _ = lf.sink(
            SinkDestination::File {
                target: SinkTarget::Path(PlRefPath::try_from_path(path.as_ref())?),
            },
            file_format,
            UnifiedSinkArgs {
                mkdir: true,
                ..Default::default()
            }
        )?
            .collect()?;

        Ok(())
    }
}
