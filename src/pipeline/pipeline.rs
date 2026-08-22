use anyhow::Result;
use polars::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::core::model::config::PipelineOutputFormat;
use crate::core::model::config::Config;
use crate::pipeline::model::config::PipelineTransformConfig;
use crate::pipeline::parser::MoonwatchLogParser;
use crate::pipeline::transform::evaluate_active_window_rules;

/// Separator placed between tags when they are flattened into one CSV field.
///
/// A pipe rather than a comma: it needs no quoting inside a comma-delimited file and is
/// very unlikely to occur inside a tag, so the field can be split again downstream.
pub const CSV_LIST_SEPARATOR: &str = "|";

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

    /// Write one flat file per event type into `pipeline_output_directory`.
    ///
    /// Parquet output carries the types of `output_active_event_schema` as they are, so
    /// `tags` stays a list and the durations stay durations. CSV has neither, so for that
    /// format `tags` is joined into one string with [`CSV_LIST_SEPARATOR`] and `duration`
    /// and `idleFor` become whole seconds - see [`Self::flatten_for_csv`]. The unlock events
    /// have no such column and come out the same either way.
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
        // An exhaustive match, so that a new output format has to make this decision rather
        // than silently inheriting "no flattening" and failing at the sink.
        let lf = match format {
            PipelineOutputFormat::Csv => Self::flatten_for_csv(lf)?,
            // Parquet stores lists and durations natively, and they are more useful that way.
            PipelineOutputFormat::Parquet => lf,
        };

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

    /// Replace the columns CSV cannot represent with flat equivalents.
    ///
    /// The CSV writer rejects `List` (and `Struct`) columns outright and has no serializer
    /// for `Duration`, so `tags`, `duration` and `idleFor` would each stop the sink. Lists
    /// are joined into a single string; durations are expressed as whole seconds, which is
    /// the unit the .jsonl logs use in the first place and the inverse of what
    /// `MoonwatchLogParser` did on the way in.
    ///
    /// Driven off the frame's own schema rather than a hard-coded list of column names, so
    /// a column added to `output_active_event_schema` later is handled without a change
    /// here.
    fn flatten_for_csv(mut lf: LazyFrame) -> Result<LazyFrame> {
        let exprs = lf.collect_schema()?
            .iter()
            .filter_map(|(name, dtype)| match dtype {
                DataType::List(_) => Some(
                    col(name.clone())
                        // `list.join` requires List(String), so make sure that is what it gets.
                        .cast(DataType::List(Box::new(DataType::String)))
                        .list()
                        // `ignore_nulls`: a null tag drops out of the joined string rather
                        // than nulling the whole field. An empty list joins to "".
                        .join(lit(CSV_LIST_SEPARATOR), true)
                        .alias(name.clone()),
                ),
                DataType::Duration(_) => Some(
                    col(name.clone()).dt().total_seconds(false).alias(name.clone()),
                ),
                _ => None,
            })
            .collect::<Vec<_>>();

        // `with_columns` replaces same-named columns in place, so the column order still
        // matches the output schema.
        Ok(lf.with_columns(exprs))
    }
}
