use std::fs;
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
    ///
    /// The buffer is only cleared once the data is safely on disk - a failed dump keeps the
    /// events, so the next attempt can still write them.
    pub fn dump(&mut self) -> Result<Option<PathBuf>> {
        if self.events.is_empty() {
            return Ok(None)
        }

        let output_dir = &self.config.log_output_subdirectory;
        if !output_dir.exists() {
            log::info!("Creating output directory {}", output_dir.display());
            fs::create_dir_all(output_dir)
                .with_context(|| format!("Failed to create {}", output_dir.display()))?;
        }

        let filename = format!("{}.jsonl", uuidv7::create());
        let output_path = output_dir.join(filename.as_str());

        log::info!("Writing {} events to {}", self.events.len(), output_path.display());
        let file = File::create(&output_path).context("Failed to create output file")?;
        let mut writer = BufWriter::new(file);

        for e in self.events.iter() {
            let mut serializer = serde_json::Serializer::new(&mut writer);
            e.serialize(&mut serializer).context("Failed to serialize event")?;
            writer.write_all(b"\n").context("Failed to write newline")?;
        }

        // This write often happens while the machine is logging off or shutting down, so get
        // the data all the way to the filesystem rather than leaving it in a buffer that a
        // Drop we never reach would have to flush. `into_inner` also surfaces a flush error,
        // which dropping the BufWriter would silently discard.
        let file = writer.into_inner().context("Failed to flush output file")?;
        file.sync_all().context("Failed to sync output file")?;

        self.events.clear();
        Ok(Some(output_path))
    }
}
