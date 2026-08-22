//! This module contains the long-running Moonwatch service: the worker thread that samples
//! and records events, the system tray icon, and the platform event loop and OS signal
//! handling that keep both alive.
//!
//! The split of responsibilities is dictated by the tray: `tray-icon` is `!Send` and both
//! backends require their event loop on the thread that created the icon, so the main thread
//! is given over to [`run_event_loop`] and everything else happens on the worker thread (see
//! [`worker::run_worker`]). The two communicate through [`worker::MoonwatcherSignal`]s in
//! one direction and [`status::SharedStatus`] in the other.

pub mod platforms;
pub mod status;
pub mod tray;
pub mod worker;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{bail, Result};
use log::{error, info, warn};

use crate::core::common::config_dir;
use crate::daemon::status::SharedStatus;
use crate::daemon::tray::{SharedOutputDir, TrayContext};
use crate::daemon::worker::{run_worker, WorkerHandle};

/// Something the event loop has to repaint when the daemon's status changes.
///
/// Implemented by [`tray::TrayHandle`]. It exists because the tray is `!Send` and can only
/// be touched from the thread running the event loop, so the loop itself has to do the
/// repainting on behalf of whichever thread changed the status.
pub trait UiRefresh {
    fn refresh(&mut self);
}

/// Run the daemon until it is asked to stop.
///
/// `config_path` points at `main_config.json`; it is read by the worker rather than here, so
/// that a configuration the user has broken shows up as a problem in the tray instead of
/// preventing the daemon (and therefore the tray) from starting at all.
pub fn run(config_path: &Path, no_tray: bool) -> Result<()> {
    let (signal_sender, signal_receiver) = crossbeam_channel::bounded(100);
    let worker = WorkerHandle::new(signal_sender);
    install_signal_handlers(worker.clone())?;

    let status = SharedStatus::new();
    // Shared so that the tray's "Open log folder" follows a configuration reload.
    let output_dir: SharedOutputDir = Arc::new(Mutex::new(None));

    let worker_thread = {
        let (status, output_dir) = (status.clone(), Arc::clone(&output_dir));
        let config_path = config_path.to_path_buf();
        thread::Builder::new()
            .name("moonwatcher-worker".to_string())
            .spawn(move || {
                let result = run_worker(config_path, signal_receiver, status, output_dir);
                if let Err(e) = &result {
                    error!("Worker stopped with an error: {e:?}");
                }
                // Nothing left for the UI thread to do once the worker is gone.
                request_ui_quit();
                result
            })?
    };

    let build_tray = if no_tray {
        info!("Running without a tray icon (--no-tray)");
        None
    } else {
        let context = TrayContext {
            worker: worker.clone(),
            status,
            output_dir,
            config_dir: config_dir(config_path),
        };
        Some(move || tray::build_tray(context))
    };

    // A missing tray is not fatal: on Linux there may be no display at all, and on Windows
    // the event loop is worth running by itself for the session-end handling.
    if let Err(e) = run_event_loop(worker.clone(), build_tray) {
        warn!("Could not run the UI event loop, continuing without a tray icon: {e:?}");
    }

    match worker_thread.join() {
        Ok(result) => result,
        Err(_) => bail!("Worker thread panicked"),
    }
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
