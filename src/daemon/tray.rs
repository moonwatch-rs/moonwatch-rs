//! System tray icon and its context menu.
//!
//! The tray runs on the UI thread (see [`crate::daemon::run_event_loop`]) and never
//! touches the event buffer itself: every menu action is turned into a
//! [`MoonwatcherSignal`] for the worker thread, or into a wait on the worker via
//! [`WorkerHandle`].
//!
//! What the icon and the menu display comes from [`SharedStatus`], which the worker writes;
//! [`TrayHandle::refresh`] is what turns a change there into pixels.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::daemon;
use crate::daemon::UiRefresh;
use crate::daemon::status::{RecordingState, SharedStatus, StatusIcon};
use crate::daemon::worker::{MoonwatcherSignal, WorkerHandle};

const RECORDING_ICON_PNG: &[u8] = include_bytes!("../../share/moonwatch-icon.png");
const PAUSED_ICON_PNG: &[u8] = include_bytes!("../../share/moonwatch-icon-paused.png");
const PROBLEM_ICON_PNG: &[u8] = include_bytes!("../../share/moonwatch-icon-error.png");

/// How long a menu action waits for the worker before giving up on it.
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

const ID_STATUS: &str = "moonwatch.status";
const ID_RELOAD: &str = "moonwatch.reload";
const ID_WRITE_NOW: &str = "moonwatch.write_now";
const ID_PAUSE: &str = "moonwatch.pause";
const ID_OPEN_LOGS: &str = "moonwatch.open_logs";
const ID_OPEN_CONFIG_DIR: &str = "moonwatch.open_config_dir";
const ID_QUIT: &str = "moonwatch.quit";

/// Where the event log is written. `None` until a configuration has been loaded, and it
/// moves if a reload points somewhere else.
pub type SharedOutputDir = Arc<Mutex<Option<PathBuf>>>;

pub struct TrayContext {
    pub worker: WorkerHandle,
    pub status: SharedStatus,
    pub output_dir: SharedOutputDir,
    /// Directory holding main_config.json and moonwatcher.log. Fixed for the process lifetime.
    pub config_dir: PathBuf,
}

/// A live tray icon plus the menu items whose text and state have to follow the daemon.
///
/// Must be kept alive: dropping it removes the icon from the tray.
pub struct TrayHandle {
    tray: TrayIcon,
    status_item: MenuItem,
    pause_item: CheckMenuItem,
    open_logs_item: MenuItem,
    status: SharedStatus,
    output_dir: SharedOutputDir,
    /// What is on screen, so a refresh that changes nothing does no work and does not log.
    displayed: Option<(StatusIcon, String)>,
}

/// Create the tray icon. Must be called on the thread running the platform event loop.
pub fn build_tray(context: TrayContext) -> Result<TrayHandle> {
    let TrayContext { worker, status, output_dir, config_dir } = context;

    // Disabled, so it reads as a heading rather than something to click. This is the only
    // textual status on Linux, where the appindicator backend ignores tooltips.
    let status_item = MenuItem::with_id(ID_STATUS, "Starting…", false, None);
    let reload = MenuItem::with_id(ID_RELOAD, "Reload configuration", true, None);
    let write_now = MenuItem::with_id(ID_WRITE_NOW, "Write events now", true, None);
    let pause_item = CheckMenuItem::with_id(ID_PAUSE, "Pause recording", true, false, None);
    let open_logs_item = MenuItem::with_id(ID_OPEN_LOGS, "Open log folder", true, None);
    let open_config_dir = MenuItem::with_id(ID_OPEN_CONFIG_DIR, "Open Moonwatch.rs folder", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "Quit Moonwatch.rs", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &reload,
        &write_now,
        &pause_item,
        &PredefinedMenuItem::separator(),
        &open_logs_item,
        &open_config_dir,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let handler_status = status.clone();
    let handler_output_dir = Arc::clone(&output_dir);
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        // Called on the UI thread, from inside the event loop, so it is safe to act
        // directly (including blocking on the worker and leaving the loop).
        match event.id.as_ref() {
            ID_RELOAD => {
                log::info!("Tray: reload configuration");
                worker.send(MoonwatcherSignal::ReloadConfig);
            }
            ID_WRITE_NOW => {
                log::info!("Tray: write events now");
                worker.flush_and_wait(WORKER_TIMEOUT);
            }
            ID_PAUSE => {
                // The worker owns this state; we only ask it to flip. Both backends have
                // already toggled the check mark by now, and the next refresh puts it back
                // in step with whatever the worker actually did.
                let pause = handler_status.get().recording != RecordingState::Paused;
                log::info!("Tray: {} recording", if pause { "pausing" } else { "resuming" });
                worker.send(MoonwatcherSignal::SetPaused(pause));
            }
            ID_OPEN_LOGS => match locked_output_dir(&handler_output_dir) {
                Some(path) => open_folder("log folder", &path),
                None => log::warn!("No log folder yet, no configuration has been loaded"),
            },
            ID_OPEN_CONFIG_DIR => open_folder("Moonwatch.rs folder", &config_dir),
            ID_QUIT => {
                log::info!("Tray: quit");
                worker.terminate_and_wait(WORKER_TIMEOUT);
                daemon::quit_event_loop();
            }
            other => log::warn!("Unexpected tray menu event: {other:?}"),
        }
    }));

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Moonwatch.rs")
        .with_icon(load_icon(PROBLEM_ICON_PNG)?)
        .build()
        .context("failed to create tray icon")?;

    let mut handle = TrayHandle {
        tray,
        status_item,
        pause_item,
        open_logs_item,
        status,
        output_dir,
        displayed: None,
    };
    // Start from the real status rather than whatever the builder happened to set.
    handle.refresh();

    Ok(handle)
}

impl UiRefresh for TrayHandle {
    fn refresh(&mut self) {
        let status = self.status.get();
        let icon = status.icon();
        let line = status.menu_line();

        let shown_icon = self.displayed.as_ref().map(|(icon, _)| *icon);
        let unchanged = self.displayed.as_ref()
            .is_some_and(|(_, shown_line)| shown_icon == Some(icon) && *shown_line == line);
        if unchanged {
            return;
        }

        // One line per state transition, and none while nothing changes: this is how a
        // problem the user only saw in the tray can still be reconstructed afterwards.
        log::info!("Tray status: {line} [{icon:?}]");

        if shown_icon != Some(icon) {
            match load_icon(icon_png(icon)) {
                Ok(image) => if let Err(e) = self.tray.set_icon(Some(image)) {
                    log::warn!("Could not show the {icon:?} tray icon: {e:?}");
                },
                Err(e) => log::warn!("Could not load the {icon:?} tray icon: {e:?}"),
            }
        }

        self.status_item.set_text(&line);
        self.pause_item.set_checked(status.recording == RecordingState::Paused);
        self.open_logs_item.set_enabled(locked_output_dir(&self.output_dir).is_some());

        // Ignored by the appindicator backend on Linux; the status item covers that.
        if let Err(e) = self.tray.set_tooltip(Some(status.tooltip())) {
            log::warn!("Could not set tray tooltip: {e:?}");
        }

        self.displayed = Some((icon, line));
    }
}

fn icon_png(icon: StatusIcon) -> &'static [u8] {
    match icon {
        StatusIcon::Recording => RECORDING_ICON_PNG,
        StatusIcon::Paused => PAUSED_ICON_PNG,
        StatusIcon::Problem => PROBLEM_ICON_PNG,
    }
}

fn locked_output_dir(output_dir: &SharedOutputDir) -> Option<PathBuf> {
    match output_dir.lock() {
        Ok(dir) => dir.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Show a directory in the system file manager, creating it if need be.
///
/// The output directory is only created by the first write, so early on it may not exist
/// yet - and Explorer answers a path that does not exist by silently opening the user's
/// Documents folder instead.
fn open_folder(what: &str, path: &Path) {
    log::info!("Tray: opening {what} {path:?}");

    if let Err(e) = std::fs::create_dir_all(path) {
        log::warn!("Could not create {path:?}: {e:?}");
    } else if let Err(e) = daemon::open_path(path) {
        log::warn!("Could not open {path:?}: {e:?}");
    }
}

fn load_icon(png: &[u8]) -> Result<Icon> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info()?;
    let info = reader.info();
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        bail!("tray icon must be 8-bit RGBA, got {:?}/{:?}", info.color_type, info.bit_depth);
    }

    let size = reader.output_buffer_size()
        .context("tray icon is too large to decode")?;
    let mut rgba = vec![0u8; size];
    let frame = reader.next_frame(&mut rgba)?;
    rgba.truncate(frame.buffer_size());

    Ok(Icon::from_rgba(rgba, frame.width, frame.height)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The icons are compiled into the binary, so a broken asset is a build-time problem
    /// rather than something the user finds out about at login.
    #[test]
    fn every_embedded_icon_decodes_to_rgba() {
        for icon in [StatusIcon::Recording, StatusIcon::Paused, StatusIcon::Problem] {
            let png = icon_png(icon);
            let mut reader = png::Decoder::new(std::io::Cursor::new(png))
                .read_info()
                .unwrap_or_else(|e| panic!("{icon:?} icon should be a valid PNG: {e}"));

            let info = reader.info();
            assert_eq!(info.color_type, png::ColorType::Rgba, "{icon:?}");
            assert_eq!(info.bit_depth, png::BitDepth::Eight, "{icon:?}");

            let size = reader.output_buffer_size().expect("known size");
            let mut rgba = vec![0u8; size];
            let frame = reader.next_frame(&mut rgba).expect("decodes");
            assert_eq!(frame.width, frame.height, "{icon:?} icon should be square");
            assert_eq!(frame.buffer_size(), (frame.width * frame.height * 4) as usize);
        }
    }

    /// The three states must not share an image, or the icon would tell the user nothing.
    #[test]
    fn the_three_icons_are_distinct() {
        assert_ne!(icon_png(StatusIcon::Recording), icon_png(StatusIcon::Paused));
        assert_ne!(icon_png(StatusIcon::Recording), icon_png(StatusIcon::Problem));
        assert_ne!(icon_png(StatusIcon::Paused), icon_png(StatusIcon::Problem));
    }
}
