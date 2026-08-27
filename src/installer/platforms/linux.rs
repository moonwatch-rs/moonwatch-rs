//! The unix side of installation: a Systemd user unit, managed with `systemctl --user`.
//!
//! A user unit rather than a system one because Moonwatch.rs records one person's session
//! and needs no privileges; installing it therefore never asks for a password.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use log::info;

use crate::core::common::home_dir;
use crate::installer::main_config_path;

/// Name of the Systemd unit, and so also of the `moonwatch-rs.service` file.
const UNIT_NAME: &str = "moonwatch-rs";

/// Stop the daemon through Systemd, so that its executable can be replaced.
///
/// `systemctl stop` does not return until the unit has actually stopped, which is what makes
/// the copy that follows safe. A failure here is not one: on a first install there is no
/// unit to stop, and the message `systemctl` prints then says so better than we could.
pub fn stop_running_instance() -> Result<()> {
    info!("Stopping the {UNIT_NAME} service, if it is running");
    match systemctl(&["stop", UNIT_NAME]) {
        Ok(_) => info!("Service stopped"),
        Err(e) => info!("Nothing was stopped: {e:#}"),
    }

    Ok(())
}

/// Write the Systemd user unit and enable it, so that Moonwatch.rs starts on login.
pub fn install_autostart(moonwatch_dir: &Path, installed_exe: &Path) -> Result<()> {
    let unit_dir = systemd_user_dir()?;
    fs::create_dir_all(&unit_dir)
        .with_context(|| format!("could not create {}", unit_dir.display()))?;

    let unit_path = unit_dir.join(format!("{UNIT_NAME}.service"));
    info!("Writing the Systemd user unit to {}", unit_path.display());
    fs::write(&unit_path, unit_file(moonwatch_dir, installed_exe))
        .with_context(|| format!("could not write {}", unit_path.display()))?;

    // The unit has just changed on disk (an upgrade may have moved the paths in it), so
    // Systemd has to be told to read it again before it is enabled or started.
    systemctl(&["daemon-reload"]).context("could not reload the Systemd user daemon")?;
    systemctl(&["enable", UNIT_NAME])
        .with_context(|| format!("could not enable the {UNIT_NAME} service"))?;

    Ok(())
}

/// Start the daemon through Systemd rather than as a child of the installer, so that the
/// process Systemd supervises is the one that ends up running.
pub fn start_daemon(_moonwatch_dir: &Path, _installed_exe: &Path) -> Result<()> {
    info!("Starting the {UNIT_NAME} service");
    systemctl(&["start", UNIT_NAME])
        .with_context(|| format!("could not start the {UNIT_NAME} service"))?;

    info!("Moonwatch.rs is running - check on it with `systemctl --user status {UNIT_NAME}`");
    Ok(())
}

/// The unit file, with the paths of this installation baked in.
///
/// Everything is quoted because Systemd splits `ExecStart` on whitespace, and an
/// installation directory with a space in it would otherwise turn into several arguments.
fn unit_file(moonwatch_dir: &Path, installed_exe: &Path) -> String {
    let config_path = main_config_path(moonwatch_dir);

    format!("\
[Unit]
Description=Moonwatch.rs daemon

[Service]
ExecStart=\"{exe}\" --config \"{config}\" watch
ExecReload=kill -HUP $MAINPID

[Install]
WantedBy=graphical-session.target
",
            exe = installed_exe.display(),
            config = config_path.display())
}

/// Where Systemd looks for a user's own units.
fn systemd_user_dir() -> Result<PathBuf> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home_dir()?.join(".config"),
    };

    Ok(config_home.join("systemd/user"))
}

/// Run `systemctl --user` with `args`, failing if it does.
fn systemctl(args: &[&str]) -> Result<()> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .with_context(|| format!("could not run `systemctl --user {}`", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`systemctl --user {}` failed ({}){}",
              args.join(" "),
              output.status,
              match stderr.trim() {
                  "" => String::new(),
                  message => format!(": {message}"),
              });
    }

    Ok(())
}
