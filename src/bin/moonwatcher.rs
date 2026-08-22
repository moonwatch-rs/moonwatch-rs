// No console on Windows, so that logging in at startup does not flash a terminal window.
// Diagnostics go to moonwatcher.log instead, see init_logging(); output for the subcommands
// that a user runs interactively is handled by attach_parent_console().
#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use flexi_logger::{Cleanup, Criterion, Duplicate, FileSpec, Logger, LoggerHandle, Naming, WriteMode};
use log::info;

use moonwatch_rs::core::common::config_dir;
use moonwatch_rs::core::model::config::{default_main_config, Config};
use moonwatch_rs::daemon;
use moonwatch_rs::pipeline::pipeline::MoonwatchPipeline;

/// Moonwatch.rs - a privacy-focused digital wellbeing app
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct MoonwatcherCli {
    /// path to main_config.json (default: next to the moonwatcher executable)
    #[arg(long, short = 'c', value_name = "MAIN_CONFIG.JSON", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the Moonwatch.rs daemon: sample the active window and record events
    Watch {
        /// run without a system tray icon
        #[arg(long)]
        no_tray: bool,
    },

    /// Run the ETL pipeline over the recorded logs and write out flat files for analysis
    Pipeline,
}

fn main() -> Result<()> {
    // Before parsing, so that --help and clap's usage errors are visible too.
    attach_parent_console();

    let cli = MoonwatcherCli::parse();
    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };

    // Held for the lifetime of the process; dropping it shuts the logger down.
    let _logger = init_logging(&config_path)?;

    info!("--- Moonwatch ---");
    info!("Configuration file: {config_path:?}");

    match cli.command {
        Command::Watch { no_tray } => daemon::run(&config_path, no_tray),
        Command::Pipeline => run_pipeline(&config_path),
    }
}

/// Read the configuration and run the ETL pipeline once.
///
/// Unlike `watch`, a configuration that cannot be read is fatal here: this is a one-shot
/// command with no tray to report the problem in and nothing useful to do without it.
fn run_pipeline(config_path: &Path) -> Result<()> {
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

/// Where the configuration lives when the user did not say.
///
/// Next to the executable, which is how Moonwatch.rs is installed on both platforms - the
/// working directory is not dependable for a process started by a login shortcut or a
/// Systemd user unit.
fn default_config_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .context("could not determine the path of the moonwatcher executable")?;
    let exe_dir = exe.parent()
        .with_context(|| format!("{} has no parent directory", exe.display()))?;

    // `default_main_config()` is the relative path config files refer to each other by
    // ("./main_config.json"); only its file name is meaningful next to the executable.
    let name = Path::new(&default_main_config()).file_name()
        .context("the default configuration name has no file name")?
        .to_owned();

    Ok(exe_dir.join(name))
}

/// Set up logging to `moonwatcher.log` next to the configuration file.
///
/// On Windows the daemon has no console of its own, so `println!` output would be lost
/// entirely, which is exactly what makes shutdown problems hard to diagnose. Messages are
/// additionally sent to stderr, where systemd picks them up into the journal on Linux.
///
/// Verbosity can be raised with eg. `MOONWATCH_LOG=debug`.
fn init_logging(config_path: &Path) -> Result<LoggerHandle> {
    let log_dir = config_dir(config_path);

    let spec = std::env::var("MOONWATCH_LOG").unwrap_or_else(|_| "info".to_string());

    let duplicate = if cfg!(debug_assertions) { Duplicate::All } else { Duplicate::Info };

    let logger = Logger::try_with_str(&spec)?
        .log_to_file(FileSpec::default()
            .directory(&log_dir)
            .basename("moonwatcher")
            .suffix("log")
            .suppress_timestamp())
        .rotate(Criterion::Size(2 * 1024 * 1024), Naming::Numbers, Cleanup::KeepLogFiles(3))
        .append()
        // Unbuffered, so that the log still explains what happened if we are killed.
        .write_mode(WriteMode::Direct)
        .format(flexi_logger::detailed_format)
        .duplicate_to_stderr(duplicate)
        .start()?;

    Ok(logger)
}

/// Write to the console of whoever launched us, if there is one.
///
/// The binary is built for the Windows subsystem so that starting at login does not flash a
/// terminal window, which also means it gets no console of its own - and `moonwatcher
/// pipeline` or `--help` would print nowhere. Attaching to the parent's console fixes that
/// for the interactive case and does nothing at login, where there is no parent console.
///
/// Note that the shell does not wait for a windows-subsystem process, so its prompt comes
/// back before our output does.
#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

    // Fails when there is no parent console (started from a shortcut, or already attached),
    // which is the normal case for the daemon and not worth reporting.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_parent_console() {}
