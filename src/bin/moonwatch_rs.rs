// No console on Windows, so that logging in at startup does not flash a terminal window.
// Diagnostics go to moonwatch_rs.log instead, see init_logging(); output for the subcommands
// that a user runs interactively is handled by attach_parent_console().
#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use flexi_logger::{Cleanup, Criterion, Duplicate, FileSpec, Logger, LoggerHandle, Naming, WriteMode};
use log::info;

use moonwatch_rs::core::common::{config_dir, moonwatch_dir_in_home};
use moonwatch_rs::core::model::config::MAIN_CONFIG_FILE_NAME;
use moonwatch_rs::installer::InstallMode;
use moonwatch_rs::{daemon, installer, pipeline};

/// Moonwatch.rs - a privacy-focused digital wellbeing app
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct MoonwatcherCli {
    /// path to main_config.json (default: next to the moonwatch_rs executable)
    #[arg(long, short = 'c', value_name = "MAIN_CONFIG.JSON", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install Moonwatch.rs into your home directory, start it, and keep starting it at login
    ///
    /// Run this on the downloaded binary; it copies itself into place. Running it again over
    /// an existing installation upgrades it: the daemon is stopped, the executable replaced
    /// and the daemon started again, while any configuration you have edited is left alone.
    /// The global --config option has no effect here, use --dir instead.
    ///
    /// Pass --files-only to prepare the directory without touching the machine you are
    /// running on: the files are written, but nothing is stopped or started and no autostart
    /// entry is made.
    Install {
        /// directory to install into (default: ~/.moonwatch-rs)
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,

        /// only write the files into the directory: do not stop or start the daemon, and do
        /// not register it to start at login
        #[arg(long)]
        files_only: bool,
    },

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

    if let Command::Install { dir, files_only } = cli.command {
        return run_install(dir, files_only);
    }

    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };

    // Held for the lifetime of the process; dropping it shuts the logger down.
    let _logger = init_logging(&config_dir(&config_path))?;

    info!("--- Moonwatch ---");
    info!("Configuration file: {config_path:?}");

    match cli.command {
        Command::Watch { no_tray } => daemon::run(&config_path, no_tray),
        Command::Pipeline => pipeline::run_pipeline(&config_path),
        // Handled above, before the logger is set up.
        Command::Install { .. } => unreachable!(),
    }
}

/// Install into `dir`, or into `~/.moonwatch-rs` when it was not given.
///
/// Logging is set up in the installation directory rather than next to the executable, which
/// at this point is wherever the binary was downloaded to. The directory has to exist for
/// that, so it is created here rather than by the installer.
///
/// `files_only` still logs to the installation directory, so a run that deliberately did
/// nothing else is diagnosable exactly like a full one.
fn run_install(dir: Option<PathBuf>, files_only: bool) -> Result<()> {
    let moonwatch_dir = match dir {
        // The autostart entry has to name the installation in a way that still means the
        // same thing when the user logs in, so a relative `--dir` is resolved now, against
        // the working directory it was written for. `absolute` rather than `canonicalize`,
        // which would need the directory to exist already.
        Some(dir) => std::path::absolute(&dir)
            .with_context(|| format!("could not resolve {}", dir.display()))?,
        None => moonwatch_dir_in_home()
            .context("pass --dir to say where Moonwatch.rs should be installed")?,
    };

    std::fs::create_dir_all(&moonwatch_dir)
        .with_context(|| format!("could not create {}", moonwatch_dir.display()))?;

    let _logger = init_logging(&moonwatch_dir)?;

    info!("--- Moonwatch ---");
    let mode = if files_only { InstallMode::FilesOnly } else { InstallMode::Full };
    installer::install(&moonwatch_dir, mode)
}

/// Where the configuration lives when the user did not say.
///
/// Next to the executable, which is how Moonwatch.rs is installed on both platforms - the
/// working directory is not dependable for a process started by a login shortcut or a
/// Systemd user unit.
fn default_config_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .context("could not determine the path of the moonwatch_rs executable")?;
    let exe_dir = exe.parent()
        .with_context(|| format!("{} has no parent directory", exe.display()))?;

    Ok(exe_dir.join(MAIN_CONFIG_FILE_NAME))
}

/// Set up logging to `moonwatch_rs.log` in `log_dir`, which is the directory holding the
/// configuration file (or, for `install`, the directory being installed into).
///
/// On Windows the daemon has no console of its own, so `println!` output would be lost
/// entirely, which is exactly what makes shutdown problems hard to diagnose. Messages are
/// additionally sent to stderr, where systemd picks them up into the journal on Linux and
/// where `install` run from a terminal reports its progress.
///
/// Verbosity can be raised with eg. `MOONWATCH_LOG=debug`.
fn init_logging(log_dir: &Path) -> Result<LoggerHandle> {
    let spec = std::env::var("MOONWATCH_LOG").unwrap_or_else(|_| "info".to_string());

    let duplicate = if cfg!(debug_assertions) { Duplicate::All } else { Duplicate::Info };

    let logger = Logger::try_with_str(&spec)?
        .log_to_file(FileSpec::default()
            .directory(log_dir)
            .basename("moonwatch_rs")
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
/// terminal window, which also means it gets no console of its own - and `moonwatch_rs
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
