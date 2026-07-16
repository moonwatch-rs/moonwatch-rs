#![windows_subsystem = "windows"]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use moonwatch_rs::watcher;
use moonwatch_rs::watcher::core::{ActiveWindowEvent, Desktop, MoonwatcherSignal};
use moonwatch_rs::watcher::config::Config;
use anyhow::Result;
use sha1::{Sha1, Digest};
use clap::Parser;

#[derive(Debug)]
enum ActiveWindowEventResult {
    DesktopLocked,
    Window { e: ActiveWindowEvent }
}

fn get_window_event(desktop: &dyn Desktop, duration: Duration) -> Result<ActiveWindowEventResult> {
    if desktop.is_screen_locked() {
        Ok(ActiveWindowEventResult::DesktopLocked)
    } else {
        let window = desktop.get_active_window()?;
        let idle_duration = desktop.get_idle_duration();
        let process_path = window.get_process_path()?;
        let window_title = window.get_title().unwrap_or_default();

        let e = ActiveWindowEvent::new(idle_duration, window_title, process_path, duration);
        Ok(ActiveWindowEventResult::Window { e })
    }
}

struct MoonwatcherWriter {
    events_to_write: Vec<ActiveWindowEvent>
}

impl MoonwatcherWriter {
    pub fn new() -> MoonwatcherWriter {
        MoonwatcherWriter {
            events_to_write: vec![]
        }
    }

    pub fn push(&mut self, e: ActiveWindowEvent) {
        self.events_to_write.push(e)
    }

    pub fn write(&mut self, config: &Config) -> Result<()> {
        if self.events_to_write.is_empty() {
            return Ok(());
        }

        // ensure output dir
        if !config.output_dir.exists() {
            println!("Creating output dir {:?}", config.output_dir);
            fs::create_dir_all(&config.output_dir)?;
        }

        // derive name for output file
        let mut hasher = Sha1::new();
        hasher.update(whoami::hostname().unwrap_or_default());
        hasher.update(whoami::username().unwrap_or_default());
        hasher.update(chrono::Utc::now().timestamp().to_le_bytes());
        hasher.update(b"moonwatcher");
        let hasher_result = hasher.finalize();
        let hex_digest: String = hasher_result.iter().map(|b| format!("{b:02x}")).collect();
        let filename = format!("{hex_digest}.jsonl");
        let output_path = config.output_dir.join(filename);

        // TODO consider writing .jsonl.gz instead
        // TODO consider allowing output encryption

        println!("Writing {} events to {:?}", self.events_to_write.len(), output_path);
        let mut fp = fs::File::create(output_path)?;
        while !self.events_to_write.is_empty() {
            let e = self.events_to_write.pop().unwrap();
            let line = e.to_json().dump();
            fp.write_all(line.as_bytes())?;
            fp.write_all(b"\n")?;
        }

        Ok(())
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// The Moonwatch.rs daemon
struct MoonwatcherCli {
    #[arg(value_name = "CONFIG.JSON", help = "path to config.json file")]
    config_path: PathBuf,
}

fn main() -> Result<()> {
    let cli = MoonwatcherCli::parse();
    let config_path = cli.config_path;

    println!("--- Moonwatch ---");
    println!("Configuration file: {:?}", config_path);
    let mut config = Config::from_file(config_path.as_path())?;
    println!("Read configuration: {:?}", config);

    let mut desktop = watcher::get_desktop(&config)?;
    println!("Using desktop implementation: {}", desktop.implementation_name());
    desktop.before_main_loop_start()?;

    let mut writer = MoonwatcherWriter::new();

    let signal_chan = watcher::get_signal_channel()?;
    let mut writer_tick_chan = crossbeam_channel::tick(config.write_every);
    let mut sample_tick_slow = false;
    let mut sample_tick_chan = crossbeam_channel::tick(config.sample_every);

    // TODO do writing in separate thread to not stall sampling

    loop {
        crossbeam_channel::select! {
            recv(signal_chan) -> sig => {
                match sig? {
                    MoonwatcherSignal::ReloadConfig => {
                        println!("Reloading configuration file");
                        match Config::from_file(config_path.as_path()) {
                            Ok(new_config) => {
                                println!("Read configuration: {:?}", new_config);

                                // in the future, Desktop may depend on Config, so reload it as well
                                match watcher::get_desktop(&new_config) {
                                    Ok(new_desktop) => {
                                        config = new_config;
                                        desktop = new_desktop;
                                        sample_tick_slow = false;
                                        sample_tick_chan = crossbeam_channel::tick(config.sample_every);
                                        writer_tick_chan = crossbeam_channel::tick(config.write_every);
                                    }
                                    Err(e) => {
                                        println!("Failed to get desktop implementation, rolling back config update: {:?}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                println!("Failed to reload configuration: {:?}", e);
                            }
                        }
                    }
                    MoonwatcherSignal::Terminate => {
                        println!("Writing data");
                        match writer.write(&config) {
                            Ok(_) => { println!("Wrote successfully"); }
                            Err(e) => { println!("Failed to write at exit, data will be lost!! Error: {:?}", e) }
                        }

                        println!("Terminating due to OS signal");
                        break;
                    }
                }
            }
            recv(writer_tick_chan) -> _ => {
                println!("Writing data");
                match writer.write(&config) {
                    Ok(_) => { println!("Wrote successfully"); }
                    Err(e) => { println!("Error when writing data (will try later): {:?}", e) }
                }
            }
            recv(sample_tick_chan) -> _ => {
                let res = get_window_event(desktop.as_ref(), config.sample_every); // this is not quite accurate w/ sample_tick_slow
                match res {
                    Ok(ActiveWindowEventResult::DesktopLocked) => {
                        if !sample_tick_slow {
                            println!("slowing down sample rate");
                            sample_tick_slow = true;
                            sample_tick_chan = crossbeam_channel::tick(10*config.sample_every);
                        }
                    }
                    Ok(ActiveWindowEventResult::Window { mut e }) => {
                        // reset sample rate
                        if sample_tick_slow {
                            println!("resetting sample rate");
                            sample_tick_slow = false;
                            sample_tick_chan = crossbeam_channel::tick(config.sample_every);
                        }

                        // do we want to skip this event?
                        let should_ignore = config.ignore.iter().any(|m| m.matches(&e));
                        if should_ignore {
                            println!("Ignoring {:?}", e);
                            continue
                        };

                        // fill in event according to config
                        e.anonymize = config.anonymize.iter().any(|m| m.matches(&e));
                        for t in &config.tags {
                            if t.matcher.matches(&e) && !e.tags.contains(&t.tag) {
                                e.tags.push_back(t.tag.clone())
                            }
                        }

                        println!("Recording {:?}", e);
                        writer.push(e);
                    }
                    _ => {
                        if !sample_tick_slow {
                            println!("slowing down sample rate");
                            sample_tick_slow = true;
                            sample_tick_chan = crossbeam_channel::tick(10*config.sample_every);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
