//! Installation of Moonwatch.rs from the `moonwatch_rs` binary itself, so that a release is
//! one file and nothing else.
//!
//! [`install`] does what the old `install_unix.py` and `install_windows.bat` did: it puts a
//! copy of the running executable into `~/.moonwatch-rs` along with default configuration,
//! registers it to start at login (a Systemd user unit on unix, the `Run` registry key on
//! Windows), and starts it.
//!
//! Re-running it is the upgrade path, which is why it begins by stopping whatever is
//! already running: a busy executable cannot be replaced on either platform, and leaving the
//! old process alive would mean two daemons sampling the same session. Configuration written
//! by an earlier install is never overwritten.

pub mod platforms;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{info, warn};

use crate::core::config_writer::ConfigWriter;
use crate::sampler;

/// Name the executable is given in the installation directory, whatever the downloaded file
/// happened to be called.
pub const EXECUTABLE_NAME: &str = if cfg!(windows) { "moonwatch_rs.exe" } else { "moonwatch_rs" };

/// Install into `moonwatch_dir` and start the daemon.
///
/// The steps are ordered around the fact that a running executable cannot be overwritten:
/// stop first, copy second. Everything is idempotent, so this doubles as the upgrade path.
pub fn install(moonwatch_dir: &Path) -> Result<()> {
    let source_exe = std::env::current_exe()
        .context("could not determine the path of the moonwatch_rs executable")?;

    info!("Installing Moonwatch.rs into {}", moonwatch_dir.display());

    // Only a warning: an installation on a machine that cannot sample right now (a headless
    // box being set up, a missing xdotool) is still worth completing.
    if let Err(e) = sampler::get_desktop() {
        warn!("This machine cannot record events yet: {e:#}");
    }

    stop_running_instance()?;
    install_files(moonwatch_dir, &source_exe)?;

    let installed_exe = moonwatch_dir.join(EXECUTABLE_NAME);
    install_autostart(moonwatch_dir, &installed_exe)?;
    start_daemon(moonwatch_dir, &installed_exe)?;

    info!("Installation finished");
    Ok(())
}

/// The part of the installation that only touches `moonwatch_dir`: the directory itself, a
/// copy of the executable, and the default configuration files and their schemas.
///
/// Split out from [`install`] so it can be tested without touching the registry, Systemd or
/// the running daemon.
pub fn install_files(moonwatch_dir: &Path, source_exe: &Path) -> Result<()> {
    fs::create_dir_all(moonwatch_dir)
        .with_context(|| format!("could not create {}", moonwatch_dir.display()))?;

    copy_executable(moonwatch_dir, source_exe)?;

    let writer = ConfigWriter::new(moonwatch_dir);
    // Schemas first and always: they describe the configuration files written below, and
    // refreshing them is how an editor gets to point out what an older config needs.
    writer.write_schemas().context("could not write the JSON schemas")?;
    // Existing configuration is deliberately kept, so that an upgrade does not throw away
    // the user's rules.
    writer.write_default_configs(false).context("could not write the default configuration")?;

    Ok(())
}

/// Copy the executable into the installation directory under its canonical name.
///
/// Does nothing when it is already there, which is what `moonwatch_rs install` run from an
/// existing installation does - copying a file onto itself would truncate it.
fn copy_executable(moonwatch_dir: &Path, source_exe: &Path) -> Result<()> {
    let target_exe = moonwatch_dir.join(EXECUTABLE_NAME);

    if is_same_file(source_exe, &target_exe) {
        info!("Already running from {}, not copying it", target_exe.display());
        return Ok(());
    }

    info!("Copying {} to {}", source_exe.display(), target_exe.display());
    // A daemon started outside the autostart entry we manage is not something the step
    // before this one knows how to stop, and an executable that is running cannot be
    // replaced on either platform - so say so, because the error on its own does not.
    fs::copy(source_exe, &target_exe)
        .with_context(|| format!("could not copy the executable to {} \
                                  (if Moonwatch.rs is still running, quit it and try again)",
                                 target_exe.display()))?;

    Ok(())
}

/// Whether both paths name the same existing file.
///
/// `canonicalize` fails for a target that does not exist yet, which is the ordinary case of
/// a first install and simply means "not the same file".
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Stop an already installed daemon, so that its executable can be replaced.
///
/// Succeeds when there was nothing running, which is the normal case for a first install.
pub fn stop_running_instance() -> Result<()> {
    #[cfg(unix)]
    return platforms::linux::stop_running_instance();

    #[cfg(windows)]
    return platforms::windows::stop_running_instance();
}

/// Arrange for `installed_exe` to be started when the user logs in.
pub fn install_autostart(moonwatch_dir: &Path, installed_exe: &Path) -> Result<()> {
    #[cfg(unix)]
    return platforms::linux::install_autostart(moonwatch_dir, installed_exe);

    #[cfg(windows)]
    return platforms::windows::install_autostart(moonwatch_dir, installed_exe);
}

/// Start the installed daemon now, rather than making the user log out and back in.
pub fn start_daemon(moonwatch_dir: &Path, installed_exe: &Path) -> Result<()> {
    #[cfg(unix)]
    return platforms::linux::start_daemon(moonwatch_dir, installed_exe);

    #[cfg(windows)]
    return platforms::windows::start_daemon(moonwatch_dir, installed_exe);
}

/// Path of the main configuration file of an installation, as the autostart entry passes it
/// to `moonwatch_rs watch`.
pub fn main_config_path(moonwatch_dir: &Path) -> PathBuf {
    moonwatch_dir.join(crate::core::model::config::MAIN_CONFIG_FILE_NAME)
}
