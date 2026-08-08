use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use crate::core::model::config::Config;
use crate::core::model::event::Event;
use anyhow::{Context, Result};
use serde::Serialize;
use crate::recorder::transform::transform_runtime_event;
use crate::sampler::model::event::RuntimeEvent;

/// Writer that takes `RuntimeEvent` objects and eventually dumps them as `Event`
/// objects into .jsonl file in a directory specified by `Config.log_output_subdirectory`.
/// The processing of `RuntimeEvent` into `Event` is defined by `RecorderConfig`.
pub struct EventRecorder<'a> {
    /// The Moonwatch configuration
    config: &'a Config,

    /// Buffer of unwritten events
    events: Vec<Event>,
}

impl EventRecorder<'_> {
    /// Create a new `EventRecorder`
    pub fn new(config: &Config) -> EventRecorder<'_> {
        EventRecorder {
            config,
            events: vec![],
        }
    }

    /// Add a new `RuntimeEvent` into the queue to be written to disk.
    /// This will eagerly transform it into `Event` (or potentially dropping it)
    /// and store it in internal buffer.
    pub fn push(&mut self, runtime_event: RuntimeEvent) -> () {
        match transform_runtime_event(&self.config.recorder_config, runtime_event) {
            None => (),
            Some(e) => {
                let event: Event = e.into();
                self.events.push(event);
            }
        };
    }

    /// Dump the `Event` queue into a new .jsonl file, clear the buffer and return output path.
    /// If the queue is empty, no file will be created and the function will return None.
    pub fn dump(&mut self) -> Result<Option<PathBuf>> {
        if self.events.is_empty() {
            return Ok(None)
        }

        let filename = format!("{}.jsonl", uuidv7::create());
        let output_path = self.config.log_output_subdirectory.join(filename.as_str());

        let file = File::create(&output_path).context("Failed to create output file")?;
        let mut writer = BufWriter::new(file);

        for e in self.events.iter() {
            let mut serializer = serde_json::Serializer::new(&mut writer);
            e.serialize(&mut serializer).context("Failed to serialize event")?;
            writer.write_all(b"\n").context("Failed to write newline")?;
        }

        self.events.clear();
        Ok(Some(output_path))
    }
}
