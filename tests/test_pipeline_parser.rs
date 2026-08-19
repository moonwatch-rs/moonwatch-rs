use std::io::Write;
use std::path::PathBuf;

use flate2::write::GzEncoder;
use flate2::Compression;
use moonwatch_rs::pipeline::model::config::{
    PipelineActiveEventAction, PipelineActiveEventPredicate, PipelineActiveEventRule,
};
use moonwatch_rs::pipeline::model::event::ActiveEventStringAttribute;
use moonwatch_rs::pipeline::parser::*;
use moonwatch_rs::pipeline::transform::evaluate_active_window_rules;
use polars::prelude::*;
use tempfile::tempdir;

mod common;

fn parser(fixtures: &[&str]) -> MoonwatchLogParser {
    let paths: Vec<PathBuf> = fixtures.iter().map(|name| common::path_to_fixture(name)).collect();
    MoonwatchLogParser::new(paths)
}

/// Return the active events of given log fixtures, ie. the input of the ETL pipeline.
fn active_events(fixtures: &[&str]) -> DataFrame {
    let lf = parser(fixtures).get_input_lazy_df().unwrap();
    MoonwatchLogParser::extract_active_event_df(lf).collect().unwrap()
}

/// Return the device unlock events of given log fixtures.
fn unlock_events(fixtures: &[&str]) -> DataFrame {
    let lf = parser(fixtures).get_input_lazy_df().unwrap();
    MoonwatchLogParser::extract_unlock_event_df(lf).collect().unwrap()
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

/// Return the `time` column formatted with given `chrono` format, asserting its dtype.
fn times_formatted(df: &DataFrame, format: &str) -> Vec<Option<String>> {
    assert_eq!(
        df.column("time").unwrap().dtype(),
        &DataType::Datetime(TimeUnit::Microseconds, None),
        "unexpected dtype of column time"
    );
    let formatted = df
        .clone()
        .lazy()
        .select([col("time").dt().to_string(format)])
        .collect()
        .unwrap();
    strings(&formatted, "time")
}

/// Return the `time` column formatted as ISO 8601, asserting its dtype.
fn times(df: &DataFrame) -> Vec<Option<String>> {
    times_formatted(df, "%Y-%m-%dT%H:%M:%S")
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
    let df = active_events(&["logs/desktop_v1.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_active_event_schema());
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
    // `processName` keeps the case of the path, `name` is lowercased
    assert_eq!(
        strings(&df, "processName"),
        vec![Some("firefox".into()), Some("Code".into()), None]
    );
    assert_eq!(
        strings(&df, "name"),
        vec![Some("firefox".into()), Some("code".into()), None]
    );
    assert_eq!(strings(&df, "hostname"), vec![Some("desktop".into()); 3]);
    assert_eq!(strings(&df, "username"), vec![Some("tom".into()); 3]);
    assert_eq!(
        tags(&df),
        vec![vec!["browser".to_string()], vec![], vec!["redacted".to_string()]]
    );

    // desktop events have no mobile attributes
    assert_eq!(strings(&df, "applicationLabel"), vec![None; 3]);
    assert_eq!(strings(&df, "applicationId"), vec![None; 3]);
    assert_eq!(bools(&df, "isMobile"), vec![Some(false); 3]);
}

/// The legacy watcher writes a flat, snake_case event with float seconds.
#[test]
fn test_active_window_event_legacy() {
    let df = active_events(&["logs/desktop_legacy.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_active_event_schema());
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
    // the second event has a Windows path, which must be split on backslashes
    assert_eq!(
        strings(&df, "processName"),
        vec![Some("vim".into()), Some("explorer".into())]
    );
    assert_eq!(strings(&df, "hostname"), vec![Some("laptop".into()); 2]);
    assert_eq!(bools(&df, "isMobile"), vec![Some(false); 2]);
}

/// The legacy watcher writes `ActiveWindowEventV1` with float seconds and with timestamps
/// rendered by `DateTime::to_rfc3339()`, which uses a `+00:00` offset and a variable number
/// of fractional second digits.
#[test]
fn test_active_window_event_v1_from_legacy_writer() {
    let df = active_events(&["logs/desktop_v1_legacy_writer.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_active_event_schema());
    assert_eq!(df.height(), 3);
    assert_eq!(
        times(&df),
        vec![
            Some("2026-08-17T13:00:00".into()),
            Some("2026-08-17T13:00:15".into()),
            Some("2026-08-17T13:00:30".into()),
        ]
    );
    // fractional seconds are kept, truncated to the microsecond time unit
    assert_eq!(
        times_formatted(&df, "%H:%M:%S%.6f"),
        vec![
            Some("13:00:00.123456".into()),
            Some("13:00:15.500000".into()),
            Some("13:00:30.000000".into()),
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
    let df = active_events(&["logs/desktop_v1.jsonl", "logs/desktop_legacy.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_active_event_schema());
    assert_eq!(df.height(), 5);
    // the events keep the order of the input files
    assert_eq!(
        strings(&df, "processName"),
        vec![
            Some("firefox".into()),
            Some("Code".into()),
            None,
            Some("vim".into()),
            Some("explorer".into()),
        ]
    );
    assert_eq!(millis(&df, "duration"), vec![Some(15_000); 5]);
}

#[test]
fn test_active_activity_event_v1() {
    let df = active_events(&["logs/mobile.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_active_event_schema());
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
    // `name` is derived from the application label of mobile events
    assert_eq!(
        strings(&df, "name"),
        vec![Some("firefox".into()), Some("signal".into())]
    );
    assert_eq!(bools(&df, "isMobile"), vec![Some(true); 2]);

    // mobile events have no desktop attributes
    assert_eq!(strings(&df, "username"), vec![None; 2]);
    assert_eq!(millis(&df, "idleFor"), vec![None; 2]);
    assert_eq!(strings(&df, "processPath"), vec![None; 2]);
    assert_eq!(strings(&df, "processName"), vec![None; 2]);
}

#[test]
fn test_device_unlock_event_v1() {
    let df = unlock_events(&["logs/mobile.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_unlock_event_schema());
    assert_eq!(df.height(), 1);
    assert_eq!(times(&df), vec![Some("2026-08-17T11:05:00".into())]);
    assert_eq!(strings(&df, "hostname"), vec![Some("pixel".into())]);
}

/// A single file may contain all event types at once, including the legacy flat one
/// whose attributes collide with the nested `data` attributes of the V1 events.
#[test]
fn test_mixed_event_types_in_one_file() {
    let active = active_events(&["logs/mixed.jsonl"]);
    assert_eq!(active.height(), 2);
    // the legacy event comes first in the file, and the events keep that order
    assert_eq!(
        strings(&active, "processName"),
        vec![Some("bash".into()), Some("htop".into())]
    );
    assert_eq!(millis(&active, "duration"), vec![Some(10_000); 2]);
    assert_eq!(tags(&active), vec![vec![], vec!["term".to_string()]]);

    let unlock = unlock_events(&["logs/mixed.jsonl"]);
    assert_eq!(unlock.height(), 1);
    assert_eq!(strings(&unlock, "hostname"), vec![Some("box".into())]);
}

/// Logs may be stored gzipped, in which case they are decompressed transparently.
#[test]
fn test_gzipped_log() {
    let tmp_dir = tempdir().unwrap();
    let gz_path = tmp_dir.path().join("desktop_v1.jsonl.gz");
    let plain = common::load_fixture("logs/desktop_v1.jsonl");

    let mut encoder = GzEncoder::new(std::fs::File::create(&gz_path).unwrap(), Compression::fast());
    encoder.write_all(plain.as_bytes()).unwrap();
    encoder.finish().unwrap();

    let lf = MoonwatchLogParser::new(vec![gz_path])
        .get_input_lazy_df()
        .unwrap();
    let from_gz = MoonwatchLogParser::extract_active_event_df(lf)
        .collect()
        .unwrap();
    let from_plain = active_events(&["logs/desktop_v1.jsonl"]);

    assert_frames_equal(&from_gz, &from_plain);
}

/// Reading a file that is not a Moonwatch log must fail rather than yield garbage.
#[test]
fn test_file_that_is_not_a_log() {
    let lf = parser(&["simple/MainConfig.json"]).get_input_lazy_df().unwrap();
    assert!(MoonwatchLogParser::extract_active_event_df(lf).collect().is_err());
}

#[test]
fn test_missing_log() {
    let lf = parser(&["logs/does_not_exist.jsonl"])
        .get_input_lazy_df()
        .unwrap();
    assert!(MoonwatchLogParser::extract_active_event_df(lf).collect().is_err());
}

#[test]
fn test_empty_log() {
    let active = active_events(&["logs/empty.jsonl"]);
    assert_eq!(active.height(), 0);
    assert_eq!(active.schema().as_ref(), &output_active_event_schema());

    let unlock = unlock_events(&["logs/empty.jsonl"]);
    assert_eq!(unlock.height(), 0);
    assert_eq!(unlock.schema().as_ref(), &output_unlock_event_schema());
}

#[test]
fn test_no_logs_at_all() {
    let active = active_events(&[]);
    assert_eq!(active.height(), 0);
    assert_eq!(active.schema().as_ref(), &output_active_event_schema());

    let unlock = unlock_events(&[]);
    assert_eq!(unlock.height(), 0);
    assert_eq!(unlock.schema().as_ref(), &output_unlock_event_schema());
}

/// Desktop and mobile active events are unified into a single dataframe,
/// where attributes that only exist on one of the two platforms are null for the other one.
#[test]
fn test_active_event_df_unifies_desktop_and_mobile() {
    let df = active_events(&["logs/desktop_v1.jsonl", "logs/mobile.jsonl"]);

    assert_eq!(df.schema().as_ref(), &output_active_event_schema());
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
fn test_active_event_df_in_pipeline() {
    let lf = parser(&["logs/desktop_v1.jsonl", "logs/mobile.jsonl"])
        .get_input_lazy_df()
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

    let out = evaluate_active_window_rules(
        MoonwatchLogParser::extract_active_event_df(lf),
        &rules,
    )
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
