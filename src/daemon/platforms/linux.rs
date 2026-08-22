//! The GTK side of the daemon: the main loop the tray icon needs, and OS signal handling.

use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread;
use std::path::Path;
use crate::daemon::UiRefresh;
use crate::daemon::worker::{MoonwatcherSignal, WorkerHandle};
use anyhow::{Context, Result};
use signal_hook::consts::{SIGHUP, TERM_SIGNALS};
use signal_hook::iterator::Signals;

/// Set by [`request_ui_refresh`] and cleared by the timer in [`run_event_loop`].
static UI_REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn install_signal_handlers(worker: WorkerHandle) -> Result<()> {
    let mut sigs = vec![SIGHUP];
    sigs.extend(TERM_SIGNALS);
    let mut signals = Signals::new(sigs)?;

    thread::spawn(move || {
        for sig in signals.forever() {
            log::info!("Received OS signal {sig:?}");
            let moonwatcher_sig = match sig {
                SIGHUP => MoonwatcherSignal::ReloadConfig,
                _ => MoonwatcherSignal::Terminate { done: None }
            };
            worker.send(moonwatcher_sig);
        }
    });

    Ok(())
}

/// Run the GTK main loop on the calling thread.
///
/// `tray-icon` builds its Linux tray on top of libappindicator, which needs GTK to be
/// initialised and a GTK main loop running on the same thread as the tray icon.
///
/// This returns `Err` when there is no usable display (no `DISPLAY`/`WAYLAND_DISPLAY`, a
/// headless session, missing libraries); the caller treats that as "run without a tray"
/// rather than as a fatal error.
pub fn run_event_loop<T: UiRefresh + 'static>(_worker: WorkerHandle,
                                              build_tray: Option<impl FnOnce() -> Result<T>>) -> Result<()> {
    // Unlike Windows, there is nothing for the loop to do here besides serving the tray.
    let Some(build_tray) = build_tray else {
        return Ok(());
    };

    gtk::init().context("failed to initialize GTK")?;

    // Keep the tray icon alive for as long as the loop runs; dropping it removes the icon.
    let tray = match build_tray() {
        Ok(tray) => Some(Rc::new(RefCell::new(tray))),
        Err(e) => {
            log::warn!("Could not create tray icon (continuing without it): {e:?}");
            None
        }
    };

    // GTK has no equivalent of posting a message to a window, so instead of waking the loop
    // from the worker thread we look at the flag once a second. The tray is `!Send`, hence
    // `timeout_add_local`, which runs the closure on this thread.
    if let Some(tray) = tray.clone() {
        let _ = gtk::glib::timeout_add_local(Duration::from_secs(1), move || {
            if UI_REFRESH_REQUESTED.swap(false, Ordering::SeqCst) {
                tray.borrow_mut().refresh();
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    log::info!("Entering GTK main loop");
    gtk::main();
    log::info!("Left GTK main loop");

    Ok(())
}

/// Leave the main loop. Must be called from the GTK thread (eg. from a tray menu callback).
pub fn quit_event_loop() {
    gtk::main_quit();
}

/// Wake the UI thread and ask it to leave the main loop. Safe to call from any thread.
pub fn request_ui_quit() {
    let _ = gtk::glib::idle_add_once(gtk::main_quit);
}

/// Ask the UI thread to repaint the tray. Safe to call from any thread; picked up by the
/// timer in [`run_event_loop`] within a second.
pub fn request_ui_refresh() {
    UI_REFRESH_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn open_path(path: &Path) -> Result<()> {
    Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}
