//! The Windows side of installation: the per-user `Run` registry key, and stopping a
//! running daemon through the hidden window it keeps for exactly this kind of message.
//!
//! `HKCU\...\Run` rather than the Startup folder that `install_windows.bat` used, because
//! writing a string to the registry needs nothing beyond the Win32 bindings the program
//! already links, whereas a `.lnk` file is a COM object someone has to build.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use log::{info, warn};
use windows::core::w;
use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT, LPARAM, WPARAM};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

/// Registry key holding the programs started when this user logs in.
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Our value under [`RUN_KEY`]. Stable, so that re-installing replaces the entry rather than
/// adding a second one.
const RUN_VALUE_NAME: windows::core::PCWSTR = w!("Moonwatch.rs");

/// Class of the hidden window the daemon creates, see `daemon::platforms::windows`.
///
/// Both sides have to agree on this string, and the daemon being stopped may well be an
/// older build, so it keeps the name it was given before the executable was renamed.
const DAEMON_WINDOW_CLASS: windows::core::PCWSTR = w!("MoonwatcherSessionWindow");

/// How long to wait for a running daemon to write out its events and exit.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to look whether it has gone.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Ask a running daemon to shut down, and wait for it to do so.
///
/// There is no service to stop on Windows, but the daemon keeps a hidden top-level window
/// whose `WM_CLOSE` handler already does the right thing: it writes out the buffered events
/// and leaves the message loop. Posting that message is therefore both how the executable is
/// freed for replacement and how an upgrade avoids losing the events recorded so far.
pub fn stop_running_instance() -> Result<()> {
    let Some(hwnd) = find_daemon_window() else {
        info!("No running Moonwatch.rs daemon found");
        return Ok(());
    };

    info!("Asking the running Moonwatch.rs daemon to write its events and exit");
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .context("could not ask the running daemon to shut down")?;

    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while Instant::now() < deadline {
        if find_daemon_window().is_none() {
            info!("The running daemon has stopped");
            return Ok(());
        }
        thread::sleep(SHUTDOWN_POLL_INTERVAL);
    }

    bail!("the running Moonwatch.rs daemon did not stop within {} seconds - \
           quit it with 'Quit Moonwatch.rs' in the tray menu and run `moonwatch_rs install` again",
          SHUTDOWN_TIMEOUT.as_secs())
}

/// The daemon's hidden window, if one exists in this session.
fn find_daemon_window() -> Option<windows::Win32::Foundation::HWND> {
    // Fails when there is no such window, which is the ordinary "nothing is running" case.
    unsafe { FindWindowW(DAEMON_WINDOW_CLASS, None) }.ok()
}

/// Register the daemon to start when this user logs in.
pub fn install_autostart(moonwatch_dir: &Path, installed_exe: &Path) -> Result<()> {
    // The whole value is one command line, so the path is quoted: `Run` entries are parsed
    // the way a command line is, and an unquoted `C:\Users\...\Program Files\...` would be
    // read as a different program plus an argument.
    let command = format!("\"{}\" watch", installed_exe.display());

    info!("Registering {} to start at login", command);
    let value = to_wide(&command);
    let size_in_bytes = u32::try_from(size_of_val(value.as_slice()))
        .context("the autostart command line is too long for the registry")?;

    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER,
                        RUN_KEY,
                        RUN_VALUE_NAME,
                        REG_SZ.0,
                        Some(value.as_ptr() as *const c_void),
                        size_in_bytes)
    }.ok().context("could not write the autostart entry to the registry")?;

    remove_legacy_startup_shortcut(moonwatch_dir);

    Ok(())
}

/// Delete the Startup folder shortcut that `install_windows.bat` used to create.
///
/// Without this, anyone upgrading from an older installation would be started twice at
/// login - once by the shortcut and once by the registry key - and end up with two daemons
/// sampling the same session. Failure is only worth a warning; the shortcut is a leftover,
/// not something this installation depends on.
fn remove_legacy_startup_shortcut(moonwatch_dir: &Path) {
    let Some(shortcut) = legacy_startup_shortcut() else { return };
    if !shortcut.exists() {
        return;
    }

    info!("Removing the Startup shortcut left by an older installation, {}",
          shortcut.display());
    if let Err(e) = std::fs::remove_file(&shortcut) {
        warn!("Could not remove {} ({e}) - delete it by hand, or Moonwatch.rs will be \
               started twice at login. The installation in {} is otherwise complete.",
              shortcut.display(), moonwatch_dir.display());
    }
}

fn legacy_startup_shortcut() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    // `moonwatcher.lnk`, not `moonwatch_rs.lnk`: this is the name the batch file wrote,
    // back when the executable was called `moonwatcher.exe`.
    Some(PathBuf::from(appdata)
        .join(r"Microsoft\Windows\Start Menu\Programs\Startup\moonwatcher.lnk"))
}

/// Start the installed daemon.
///
/// Detached, in its own process group, and with its standard streams closed. `moonwatch_rs
/// install` is normally run from a console, and a daemon that inherited it would be killed
/// along with it when the user closes that window - and until then would keep printing its
/// own log into a terminal the user has moved on from. The daemon's diagnostics go to
/// `moonwatch_rs.log` in the installation directory instead.
pub fn start_daemon(moonwatch_dir: &Path, installed_exe: &Path) -> Result<()> {
    info!("Starting {}", installed_exe.display());

    // Handle inheritance on Windows is per-process, not per-handle-argument: `CreateProcess`
    // is asked to inherit, and then *every* inheritable handle we hold goes to the child,
    // including the ones the shell handed us. A daemon that outlives the installer while
    // holding the write end of a pipe means `moonwatch_rs install | tee` never sees end of
    // input and hangs, so those handles are made non-inheritable first. The `Stdio::null()`
    // below is what the daemon then actually gets.
    make_standard_handles_non_inheritable();

    Command::new(installed_exe)
        .arg("watch")
        .current_dir(moonwatch_dir)
        .creation_flags(DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {}", installed_exe.display()))?;

    info!("Moonwatch.rs is running - look for the moon icon in the notification area \
           (on Windows 11, behind the '^' button until you drag it out)");
    Ok(())
}

/// Clear the inherit flag on our own standard handles, so that a child process cannot end
/// up holding them.
///
/// Each call is allowed to fail and is not worth reporting: there is nothing to clear when a
/// handle is closed or absent, which is the ordinary case for the daemon started at login.
fn make_standard_handles_non_inheritable() {
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let Ok(handle) = (unsafe { GetStdHandle(id) }) else { continue };
        if handle.is_invalid() {
            continue;
        }
        let _ = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
    }
}

/// A NUL-terminated UTF-16 string, as the registry stores a `REG_SZ`.
fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}
