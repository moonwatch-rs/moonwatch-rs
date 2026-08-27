use std::iter::zip;
use moonwatch_rs::sampler::model::event::RuntimeEvent;
use serde_json::Deserializer;
use moonwatch_rs::core::model::event::Event;
use moonwatch_rs::recorder::model::config::RecorderConfig;
use moonwatch_rs::recorder::transform::transform_runtime_event;
use crate::common::path_to_fixture;
use tempfile::tempdir;
use moonwatch_rs::core::model::config::Config;
use moonwatch_rs::recorder::recorder::EventRecorder;

mod common;

fn read_runtime_events(name: &str) -> Vec<RuntimeEvent> {
    let input = common::load_fixture(name);
    let stream = Deserializer::from_str(input.as_str()).into_iter::<RuntimeEvent>();
    stream.map(|x| x.unwrap()).collect()
}

fn read_events(name: &str) -> Vec<Event> {
    let input = common::load_fixture(name);
    let stream = Deserializer::from_str(input.as_str()).into_iter::<Event>();
    stream.map(|x| x.unwrap()).collect()
}

fn read_recorder_config(name: &str) -> RecorderConfig {
    let input = common::load_fixture(name);
    serde_json::from_str(&input).unwrap()
}

#[test]
fn test_transform_runtime_event() {
    let input_events = read_runtime_events("simple/RuntimeEvent_input.jsonl");
    let output_events_reference = read_runtime_events("simple/RuntimeEvent_output.jsonl");
    let config = read_recorder_config("simple/RecorderConfig.json");

    let output_events: Vec<RuntimeEvent> = input_events
        .iter()
        .map(|e| transform_runtime_event(&config, e.clone()))
        .flatten()
        .collect();

    for e in &output_events {
        println!("{}", serde_json::to_string(&e).unwrap());
    }

    for (x, y) in zip(output_events, output_events_reference) {
        assert_eq!(x, y);
    }
}

#[test]
fn test_event_recorder() {
    let input_events = read_runtime_events("simple/RuntimeEvent_input.jsonl");
    let output_events_reference = read_events("simple/Event_output.jsonl");
    let mut config = Config::from_file(path_to_fixture("simple/MainConfig.json").as_path()).unwrap();
    let tmp_dir = tempdir().unwrap();

    config.log_directory = tmp_dir.path().into();
    config.log_output_subdirectory = config.log_directory.clone();

    let mut recorder = EventRecorder::new(&config);
    for e in input_events {
        recorder.push(e);
    }
    let output_path = recorder.dump().unwrap().unwrap();

    let output_events: Vec<Event> = Deserializer::from_str(std::fs::read_to_string(output_path).unwrap().as_str())
        .into_iter::<Event>()
        .flatten()
        .collect();

    for (x, y) in zip(output_events, output_events_reference) {
        assert_eq!(x, y);
    }

    // now the buffer is empty, recorder won't create a new file
    match recorder.dump() {
        Ok(None) => {}
        _ => assert!(false)
    };
}

/// The daemon writes into a `logDirectory` that may not exist yet - on a fresh install
/// nothing has created it, and a reload can point at somewhere new.
#[test]
fn test_event_recorder_creates_a_missing_output_directory() {
    let input_events = read_runtime_events("simple/RuntimeEvent_input.jsonl");
    let mut config = Config::from_file(path_to_fixture("simple/MainConfig.json").as_path()).unwrap();
    let tmp_dir = tempdir().unwrap();

    // A subdirectory that does not exist yet, two levels deep.
    config.log_output_subdirectory = tmp_dir.path().join("logs").join("this-host");
    assert!(!config.log_output_subdirectory.exists());

    let mut recorder = EventRecorder::new(&config);
    for e in input_events {
        recorder.push(e);
    }
    let output_path = recorder.dump().unwrap().unwrap();

    assert!(output_path.starts_with(&config.log_output_subdirectory));
    assert!(std::fs::read_to_string(&output_path).unwrap().contains("ActiveWindowEventV1"));
}
