//! Tests of the files `MoonwatchPipeline::write` produces.
//!
//! The point of interest is that CSV cannot represent everything the pipeline's schema uses -
//! `tags` is a list and the durations are durations - so the CSV output is flattened while the
//! parquet output keeps its native types.

use std::path::PathBuf;

use moonwatch_rs::core::model::config::PipelineOutputFormat;
use moonwatch_rs::pipeline::model::config::{
    PipelineActiveEventAction, PipelineActiveEventPredicate, PipelineActiveEventRule,
    PipelineTransformConfig,
};
use moonwatch_rs::pipeline::parser::MoonwatchLogParser;
use moonwatch_rs::pipeline::pipeline::{MoonwatchPipeline, CSV_LIST_SEPARATOR};
use polars::prelude::*;
use tempfile::{tempdir, TempDir};

mod common;

/// A pipeline over the desktop and mobile log fixtures, writing into a fresh directory.
///
/// One rule tags the browser events a second time, so that at least one row has two tags and
/// the CSV output actually exercises the separator rather than only the single-tag case.
fn pipeline(format: PipelineOutputFormat) -> (MoonwatchPipeline, TempDir) {
    let mut config = PipelineTransformConfig::new();
    config.active_event_rules.push(PipelineActiveEventRule {
        predicate: PipelineActiveEventPredicate::HasTag("browser".to_string()),
        actions: vec![PipelineActiveEventAction::AddTag("web".to_string())],
    });

    let output_dir = tempdir().unwrap();
    let pipeline = MoonwatchPipeline {
        config,
        parser: MoonwatchLogParser::new(vec![
            common::path_to_fixture("logs/desktop_v1.jsonl"),
            common::path_to_fixture("logs/mobile.jsonl"),
        ]),
        pipeline_output_directory: output_dir.path().to_path_buf(),
        pipeline_output_format: format,
    };

    (pipeline, output_dir)
}

fn output_path(dir: &TempDir, name: &str, format: PipelineOutputFormat) -> PathBuf {
    dir.path().join(format!("{name}{}", format.get_file_extension()))
}

fn read_csv(path: &PathBuf) -> DataFrame {
    LazyCsvReader::new(PlRefPath::try_from_path(path).unwrap())
        .finish()
        .unwrap()
        .collect()
        .unwrap()
}

fn read_parquet(path: &PathBuf) -> DataFrame {
    LazyFrame::scan_parquet(PlRefPath::try_from_path(path).unwrap(), Default::default())
        .unwrap()
        .collect()
        .unwrap()
}

fn strings(df: &DataFrame, name: &str) -> Vec<Option<String>> {
    df.column(name).unwrap().str().unwrap().iter()
        .map(|x| x.map(String::from))
        .collect()
}

fn ints(df: &DataFrame, name: &str) -> Vec<Option<i64>> {
    df.column(name).unwrap().cast(&DataType::Int64).unwrap()
        .i64().unwrap().iter().collect()
}

fn dtype(df: &DataFrame, name: &str) -> DataType {
    df.column(name).unwrap().dtype().clone()
}

/// The bug this exists for: `tags` used to stop the CSV sink outright ("CSV format does not
/// support nested data"), and the durations behind it ("datatype duration[ms] cannot be
/// written to CSV").
#[test]
fn csv_output_is_flat() {
    let (pipeline, dir) = pipeline(PipelineOutputFormat::Csv);
    pipeline.write().expect("writing CSV should succeed");

    let df = read_csv(&output_path(&dir, "active_events", PipelineOutputFormat::Csv));

    assert_eq!(dtype(&df, "tags"), DataType::String, "tags must not stay a list");
    assert_eq!(
        strings(&df, "tags"),
        vec![
            // Tagged "browser" in the log, then "web" by the rule above.
            Some(format!("browser{CSV_LIST_SEPARATOR}web")),
            // No tags at all: an empty field rather than a null or "[]".
            Some(String::new()),
            Some("redacted".to_string()),
            // Mobile events carry no tags at all; the parser coalesces that to an empty
            // list, so they flatten to an empty field too rather than to null.
            Some(String::new()),
            Some(String::new()),
        ]
    );

    // Whole seconds, in the same units as the .jsonl input.
    assert_eq!(ints(&df, "duration"), vec![Some(15), Some(15), Some(15), Some(137), Some(60)]);
    assert_eq!(ints(&df, "idleFor"), vec![Some(0), Some(300), Some(0), None, None]);
}

/// A regression here would fail the sink rather than an assertion above, so check the bytes:
/// no list syntax, and the tags of one row really are in one field.
#[test]
fn csv_output_has_no_nested_syntax() {
    let (pipeline, dir) = pipeline(PipelineOutputFormat::Csv);
    pipeline.write().unwrap();

    let text = std::fs::read_to_string(
        output_path(&dir, "active_events", PipelineOutputFormat::Csv)).unwrap();

    assert!(text.starts_with("time,duration,hostname,"), "got {:?}", text.lines().next());
    assert!(text.contains(&format!("browser{CSV_LIST_SEPARATOR}web")), "got {text}");
    for nested in ['[', ']'] {
        assert!(!text.contains(nested), "{nested:?} should not appear in CSV: {text}");
    }
}

/// The unlock events have no list or duration column, so they are written the same either
/// way - but they still have to be written.
#[test]
fn unlock_events_are_written_as_csv() {
    let (pipeline, dir) = pipeline(PipelineOutputFormat::Csv);
    pipeline.write().unwrap();

    let df = read_csv(&output_path(&dir, "unlock_events", PipelineOutputFormat::Csv));

    assert_eq!(df.height(), 1);
    assert_eq!(strings(&df, "hostname"), vec![Some("pixel".to_string())]);
}

/// Flattening is for CSV only: parquet can hold both types, and they are more useful that
/// way. This guards against the flattening being moved up into `write` later on.
#[test]
fn parquet_output_keeps_native_types() {
    let (pipeline, dir) = pipeline(PipelineOutputFormat::Parquet);
    pipeline.write().expect("writing parquet should succeed");

    let df = read_parquet(&output_path(&dir, "active_events", PipelineOutputFormat::Parquet));

    assert_eq!(dtype(&df, "tags"), DataType::List(Box::new(DataType::String)));
    assert!(matches!(dtype(&df, "duration"), DataType::Duration(_)),
            "got {:?}", dtype(&df, "duration"));
    assert!(matches!(dtype(&df, "idleFor"), DataType::Duration(_)));
}
