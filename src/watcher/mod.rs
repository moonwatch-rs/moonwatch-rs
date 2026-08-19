pub mod core;
pub mod platforms;
pub mod config;
pub mod status;
pub mod tray;

use std::path::Path;
use anyhow::Result;
use crate::watcher::config::Config;
use crate::watcher::core::{Desktop, WorkerHandle};

/// Something the event loop has to repaint when the daemon's status changes.
///
/// Implemented by [`tray::TrayHandle`]. It exists because the tray is `!Send` and can only
/// be touched from the thread running the event loop, so the loop itself has to do the
/// repainting on behalf of whichever thread changed the status.
pub trait UiRefresh {
    fn refresh(&mut self);
}

pub fn get_desktop(config: &Config) -> Result<Box<dyn core::Desktop>> {
    #[cfg(unix)]
    fn get_desktop_impl(_config: &Config) -> Result<Box<dyn core::Desktop>> {
        // TODO support more UNIX platforms, possibly use config to request a particular impl.
        
        let desktop = Box::new(platforms::linux::GnomeDesktop);
        
        desktop.check_implementation_available()?;
        Ok(desktop)
    }

    #[cfg(windows)]
    fn get_desktop_impl(_config: &Config) -> Result<Box<dyn core::Desktop>> {
        let desktop = Box::new(platforms::windows::WindowsDesktop);
        desktop.check_implementation_available()?;
        Ok(desktop)
    }

    get_desktop_impl(config)
}

/// Arrange for OS signals (SIGHUP/SIGTERM on unix, Ctrl-C on Windows) to be forwarded to
/// the worker thread.
pub fn install_signal_handlers(worker: WorkerHandle) -> Result<()> {
    #[cfg(unix)]
    return platforms::linux::install_signal_handlers(worker);

    #[cfg(windows)]
    return platforms::windows::install_signal_handlers(worker);
}

/// Run the platform UI event loop (GTK on unix, Win32 messages on Windows) on the calling
/// thread, creating the tray icon on that same thread as both backends require.
///
/// Pass `None` to skip the tray icon. On Windows the loop still runs, because that is also
/// what delivers the session-end messages; on unix there is then nothing to do and this
/// returns immediately.
///
/// Returns `Err` if no event loop could be started at all, which the caller should treat
/// as "keep watching without a tray icon".
// The `'static` bound is what the unix backend needs: it keeps the tray alive in an `Rc`
// for as long as the GTK loop runs. Windows does not need it, but the wrapper has to
// satisfy both.
pub fn run_event_loop<T: UiRefresh + 'static>(worker: WorkerHandle,
                                              build_tray: Option<impl FnOnce() -> Result<T>>) -> Result<()> {
    #[cfg(unix)]
    return platforms::linux::run_event_loop(worker, build_tray);

    #[cfg(windows)]
    return platforms::windows::run_event_loop(worker, build_tray);
}

/// Ask the UI thread to repaint from the current [`status::SharedStatus`]. Safe to call from
/// any thread; a no-op when no event loop is running.
pub fn request_ui_refresh() {
    #[cfg(unix)]
    platforms::linux::request_ui_refresh();

    #[cfg(windows)]
    platforms::windows::request_ui_refresh();
}

/// Leave the event loop. Must be called from the UI thread.
pub fn quit_event_loop() {
    #[cfg(unix)]
    platforms::linux::quit_event_loop();

    #[cfg(windows)]
    platforms::windows::quit_event_loop();
}

/// Ask the UI thread to leave its event loop. Safe to call from any thread.
pub fn request_ui_quit() {
    #[cfg(unix)]
    platforms::linux::request_ui_quit();

    #[cfg(windows)]
    platforms::windows::request_ui_quit();
}

/// Open a directory in the system file manager.
pub fn open_path(path: &Path) -> Result<()> {
    #[cfg(unix)]
    return platforms::linux::open_path(path);

    #[cfg(windows)]
    return platforms::windows::open_path(path);
}
