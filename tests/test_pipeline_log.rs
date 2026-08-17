use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;
use moonwatch_rs::pipeline::log::*;
use moonwatch_rs::pipeline::model::config::{
    PipelineActiveEventAction, PipelineActiveEventPredicate, PipelineActiveEventRule,
};
use moonwatch_rs::pipeline::model::event::ActiveEventStringAttribute;
use moonwatch_rs::pipeline::transform::evaluate_active_window_rules;
use polars::prelude::*;
use tempfile::tempdir;

mod common;

fn parse(fixtures: &[&str]) -> MoonwatchLogParser {
    let logs: Vec<MoonwatchLog> = fixtures
        .iter()
        .map(|name| MoonwatchLog::new(common::path_to_fixture(name)))
        .collect();
    MoonwatchLogParser::read(&logs).unwrap()
}

fn strings(df: &DataFrame, name: &str) -> Vec<Option<String>> {
    df.column(name)
        .unwrap()
        .str()
        .unwrap()
        .iter()
        .map(|x| x.map(String::from))
        .collect()
}

fn bools(df: &DataFrame, name: &str) -> Vec<Option<bool>> {
    df.column(name).unwrap().bool().unwrap().iter().collect()
}

/// Return values of a duration column in milliseconds, asserting its time unit.
fn millis(df: &DataFrame, name: &str) -> Vec<Option<i64>> {
    let column = df.column(name).unwrap();
    assert_eq!(
        column.dtype(),
        &DataType::Duration(TimeUnit::Milliseconds),
        "unexpected dtype of column {name}"
    );
    column
        .as_materialized_series()
        .to_physical_repr()
        .i64()
        .unwrap()
        .iter()
        .collect()
}

/// Return the `time` column formatted as ISO 8601, asserting its dtype.
fn times(df: &DataFrame) -> Vec<Option<String>> {
    assert_eq!(
        df.column("time").unwrap().dtype(),
        &DataType::Datetime(TimeUnit::Microseconds, None),
        "unexpected dtype of column time"
    );
    let formatted = df
        .clone()
        .lazy()
        .select([col("time").dt().to_string("%Y-%m-%dT%H:%M:%S")])
        .collect()
        .unwrap();
    strings(&formatted, "time")
}

/// Return values of a list-of-string column, with nulls in the outer list mapped to `None`.
fn string_lists(df: &DataFrame, name: &str) -> Vec<Option<Vec<Option<String>>>> {
    let column = df.column(name).unwrap();
    let lists = column.list().unwrap();
    (0..lists.len())
        .map(|i| {
            lists.get_as_series(i).map(|series| {
                series
                    .str()
                    .unwrap()
                    .iter()
                    .map(|x| x.map(String::from))
                    .collect()
            })
        })
        .collect()
}

fn assert_frames_equal(left: &DataFrame, right: &DataFrame) {
    assert_eq!(left.schema(), right.schema());
    assert_eq!(left.height(), right.height());
    for (x, y) in left.columns().iter().zip(right.columns()) {
        assert!(x.equals_missing(y), "column {} differs", x.name());
    }
}

fn tags(df: &DataFrame) -> Vec<Vec<String>> {
    string_lists(df, "tags")
        .into_iter()
        .map(|item| {
            item.expect("tags must not be null")
                .into_iter()
                .map(|x| x.expect("individual tags must not be null"))
                .collect()
        })
        .collect()
}

#[test]
fn test_active_window_event_v1() {
    let df = parse(&["logs/desktop_v1.jsonl"])
        .active_window_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &active_window_event_schema());
    assert_eq!(df.height(), 3);
    assert_eq!(
        times(&df),
        vec![
            Some("2026-08-17T09:00:00".into()),
            Some("2026-08-17T09:00:15".into()),
            Some("2026-08-17T09:00:30".into()),
        ]
    );
    assert_eq!(millis(&df, "duration"), vec![Some(15_000); 3]);
    assert_eq!(
        millis(&df, "idleFor"),
        vec![Some(0), Some(300_000), Some(0)]
    );
    assert_eq!(
        strings(&df, "processPath"),
        vec![
            Some("C:\\Program Files\\Mozilla Firefox\\firefox.exe".into()),
            Some("/usr/bin/Code".into()),
            None,
        ]
    );
    assert_eq!(
        strings(&df, "processName"),
        vec![Some("firefox".into()), Some("code".into()), None]
    );
    assert_eq!(strings(&df, "hostname"), vec![Some("desktop".into()); 3]);
    assert_eq!(strings(&df, "username"), vec![Some("tom".into()); 3]);
    assert_eq!(
        tags(&df),
        vec![vec!["browser".to_string()], vec![], vec!["redacted".to_string()]]
    );
}

/// The legacy watcher writes a flat, snake_case event with float seconds.
#[test]
fn test_active_window_event_legacy() {
    let df = parse(&["logs/desktop_legacy.jsonl"])
        .active_window_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &active_window_event_schema());
    assert_eq!(df.height(), 2);
    // the legacy watcher writes the UTC offset as `+00:00` rather than as `Z`
    assert_eq!(
        times(&df),
        vec![
            Some("2026-08-17T10:00:00".into()),
            Some("2026-08-17T10:00:15".into()),
        ]
    );
    assert_eq!(millis(&df, "duration"), vec![Some(15_000); 2]);
    assert_eq!(millis(&df, "idleFor"), vec![Some(3_000), Some(0)]);
    assert_eq!(
        strings(&df, "processName"),
        vec![Some("vim".into()), Some("explorer".into())]
    );
    assert_eq!(strings(&df, "hostname"), vec![Some("laptop".into()); 2]);
}

/// The legacy watcher writes `ActiveWindowEventV1` with float seconds and with timestamps
/// rendered by `DateTime::to_rfc3339()`, which uses a `+00:00` offset and a variable number
/// of fractional second digits.
#[test]
fn test_active_window_event_v1_from_legacy_writer() {
    let df = parse(&["logs/desktop_v1_legacy_writer.jsonl"])
        .active_window_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &active_window_event_schema());
    assert_eq!(df.height(), 3);
    assert_eq!(
        times(&df),
        vec![
            Some("2026-08-17T13:00:00".into()),
            Some("2026-08-17T13:00:15".into()),
            Some("2026-08-17T13:00:30".into()),
        ]
    );
    assert_eq!(millis(&df, "duration"), vec![Some(15_000); 3]);
    assert_eq!(millis(&df, "idleFor"), vec![Some(3_000), Some(0), Some(0)]);
    assert_eq!(
        strings(&df, "processName"),
        vec![Some("notepad".into()), None, Some("gedit".into())]
    );
    assert_eq!(tags(&df), vec![vec!["editor".to_string()], vec![], vec![]]);
}

/// Logs written by different Moonwatch versions must end up in the same dataframe.
#[test]
fn test_active_window_event_v1_and_legacy_together() {
    let df = parse(&["logs/desktop_v1.jsonl", "logs/desktop_legacy.jsonl"])
        .active_window_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &active_window_event_schema());
    assert_eq!(df.height(), 5);
    assert_eq!(
        strings(&df, "processName"),
        vec![
            Some("firefox".into()),
            Some("code".into()),
            None,
            Some("vim".into()),
            Some("explorer".into()),
        ]
    );
}

#[test]
fn test_active_activity_event_v1() {
    let df = parse(&["logs/mobile.jsonl"])
        .active_activity_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &active_activity_event_schema());
    assert_eq!(df.height(), 2);
    assert_eq!(millis(&df, "duration"), vec![Some(137_000), Some(60_000)]);
    assert_eq!(strings(&df, "hostname"), vec![Some("pixel".into()); 2]);
    assert_eq!(
        strings(&df, "applicationLabel"),
        vec![Some("Firefox".into()), Some("Signal".into())]
    );
    assert_eq!(
        strings(&df, "applicationId"),
        vec![
            Some("org.mozilla.firefox".into()),
            Some("org.thoughtcrime.securesms".into()),
        ]
    );
}

#[test]
fn test_device_unlock_event_v1() {
    let df = parse(&["logs/mobile.jsonl"])
        .device_unlock_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &device_unlock_event_schema());
    assert_eq!(df.height(), 1);
    assert_eq!(strings(&df, "hostname"), vec![Some("pixel".into())]);
}

/// A single file may contain all event types at once, including the legacy flat one
/// whose attributes collide with the nested `data` attributes of the V1 events.
#[test]
fn test_mixed_event_types_in_one_file() {
    let parser = parse(&["logs/mixed.jsonl"]);

    let active = parser.active_window_event_df().unwrap();
    assert_eq!(active.height(), 2);
    assert_eq!(
        strings(&active, "processName"),
        vec![Some("htop".into()), Some("bash".into())]
    );
    assert_eq!(millis(&active, "duration"), vec![Some(10_000); 2]);

    let unlock = parser.device_unlock_event_df().unwrap();
    assert_eq!(unlock.height(), 1);
    assert_eq!(strings(&unlock, "hostname"), vec![Some("box".into())]);

    // there are no mobile activity events in this file
    let activity = parser.active_activity_event_df().unwrap();
    assert_eq!(activity.height(), 0);
    assert_eq!(activity.schema().as_ref(), &active_activity_event_schema());
}

#[test]
fn test_gzipped_log() {
    let tmp_dir = tempdir().unwrap();
    let gz_path = tmp_dir.path().join("desktop_v1.jsonl.gz");
    let plain = common::load_fixture("logs/desktop_v1.jsonl");

    let mut encoder = GzEncoder::new(std::fs::File::create(&gz_path).unwrap(), Compression::fast());
    encoder.write_all(plain.as_bytes()).unwrap();
    encoder.finish().unwrap();

    let from_gz = MoonwatchLogParser::read(&[MoonwatchLog::new(&gz_path)])
        .unwrap()
        .active_window_event_df()
        .unwrap();
    let from_plain = parse(&["logs/desktop_v1.jsonl"])
        .active_window_event_df()
        .unwrap();

    assert_frames_equal(&from_gz, &from_plain);
}

#[test]
fn test_unsupported_file_extension() {
    let result = MoonwatchLogParser::read(&[MoonwatchLog::new(common::path_to_fixture(
        "simple/MainConfig.json",
    ))]);
    assert!(result.is_err());
}

#[test]
fn test_empty_log() {
    let parser = parse(&["logs/empty.jsonl"]);

    for df in [
        parser.active_window_event_df().unwrap(),
        parser.active_activity_event_df().unwrap(),
        parser.device_unlock_event_df().unwrap(),
        parser.unified_active_event_df().unwrap(),
    ] {
        assert_eq!(df.height(), 0);
    }

    assert_eq!(
        parser.unified_active_event_df().unwrap().schema().as_ref(),
        &unified_active_event_schema()
    );
}

#[test]
fn test_no_logs_at_all() {
    let parser = MoonwatchLogParser::read(&[]).unwrap();
    let df = parser.unified_active_event_df().unwrap();

    assert_eq!(df.height(), 0);
    assert_eq!(df.schema().as_ref(), &unified_active_event_schema());
}

#[test]
fn test_find_in_directory() {
    let logs = MoonwatchLog::find_in_directory(&common::path_to_fixture("logs")).unwrap();
    let names: Vec<String> = logs
        .iter()
        .map(|log| log.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert_eq!(
        names,
        vec![
            "desktop_legacy.jsonl",
            "desktop_v1.jsonl",
            "desktop_v1_legacy_writer.jsonl",
            "empty.jsonl",
            "mixed.jsonl",
            "mobile.jsonl",
        ]
    );
}

#[test]
fn test_unified_active_event_df() {
    let df = parse(&["logs/desktop_v1.jsonl", "logs/mobile.jsonl"])
        .unified_active_event_df()
        .unwrap();

    assert_eq!(df.schema().as_ref(), &unified_active_event_schema());
    assert_eq!(df.height(), 5);

    // desktop events come first, then mobile ones
    assert_eq!(
        bools(&df, "isMobile"),
        vec![Some(false), Some(false), Some(false), Some(true), Some(true)]
    );
    assert_eq!(bools(&df, "ignore"), vec![Some(false); 5]);
    assert_eq!(
        strings(&df, "name"),
        vec![
            Some("firefox".into()),
            Some("code".into()),
            None,
            Some("firefox".into()),
            Some("signal".into()),
        ]
    );
    assert_eq!(strings(&df, "category"), vec![None; 5]);

    // desktop-only attributes
    assert_eq!(
        strings(&df, "username"),
        vec![Some("tom".into()), Some("tom".into()), Some("tom".into()), None, None]
    );
    assert_eq!(
        millis(&df, "idleFor"),
        vec![Some(0), Some(300_000), Some(0), None, None]
    );

    // mobile-only attributes
    assert_eq!(
        strings(&df, "applicationLabel"),
        vec![None, None, None, Some("Firefox".into()), Some("Signal".into())]
    );

    // mobile events have no tags, but the list must be empty rather than null
    // so that the pipeline can still add tags to them
    assert_eq!(
        tags(&df),
        vec![
            vec!["browser".to_string()],
            vec![],
            vec!["redacted".to_string()],
            vec![],
            vec![],
        ]
    );
}

/// The dataframe must be directly usable as input of the ETL pipeline.
#[test]
fn test_unified_active_event_df_in_pipeline() {
    let df = parse(&["logs/desktop_v1.jsonl", "logs/mobile.jsonl"])
        .unified_active_event_df()
        .unwrap();

    let rules = vec![
        PipelineActiveEventRule {
            predicate: PipelineActiveEventPredicate::IdleForGreaterThanSec(60),
            actions: vec![PipelineActiveEventAction::Ignore],
        },
        PipelineActiveEventRule {
            predicate: PipelineActiveEventPredicate::IsMobile,
            actions: vec![PipelineActiveEventAction::AddTag("mobile".to_string())],
        },
        PipelineActiveEventRule {
            predicate: PipelineActiveEventPredicate::HasTag("browser".to_string()),
            actions: vec![PipelineActiveEventAction::SetAttribute {
                name: ActiveEventStringAttribute::Category,
                value: "web".to_string(),
            }],
        },
    ];

    let out = evaluate_active_window_rules(df.lazy(), &rules)
        .collect()
        .unwrap();

    // only the event with idleFor = 300 s is ignored
    assert_eq!(
        bools(&out, "ignore"),
        vec![Some(false), Some(true), Some(false), Some(false), Some(false)]
    );
    assert_eq!(
        tags(&out),
        vec![
            vec!["browser".to_string()],
            vec![],
            vec!["redacted".to_string()],
            vec!["mobile".to_string()],
            vec!["mobile".to_string()],
        ]
    );
    assert_eq!(
        strings(&out, "category"),
        vec![Some("web".into()), None, None, None, None]
    );
}

// #[test]
// fn test_foo() {
//     let logs = MoonwatchLog::find_in_directory(std::path::Path::new(r"C:\Users\xxx")).unwrap();
//     let parser = MoonwatchLogParser::read(logs.as_slice()).unwrap();
//     let active_df = parser.unified_active_event_df().unwrap();
//     println!("{}", active_df.height());
// }
