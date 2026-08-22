//! The desktop session as Moonwatch needs to see it, and nothing more.
//!
//! Sampling only ever asks three things of the operating system: whether the screen is
//! locked, how long the user has been idle, and which window has focus. Everything
//! platform-specific lives behind these two traits, in [`crate::sampler::platforms`].

use std::path::PathBuf;
use std::time::Duration;
use anyhow::Result;

pub trait Window {
    fn get_title(&self) -> Result<String>;
    fn get_process_id(&self) -> Result<u64>;
    fn get_process_path(&self) -> Result<PathBuf>;
}

pub trait Desktop {
    fn implementation_name(&self) -> &'static str;
    fn check_implementation_available(&self) -> Result<()> {
        Ok(())
    }

    /// Whether the screen is locked. Implementations degrade to `false` rather than
    /// reporting an error, because a missing or idle screensaver is not a malfunction.
    fn is_screen_locked(&self) -> bool;

    /// How long the user has been idle.
    ///
    /// `Err` means the implementation could not find out. That is never routine, so it is
    /// reported to the user - unlike an infallible version, which would report zero idle
    /// time (ie. "the user is active") whenever it failed.
    fn get_idle_duration(&self) -> Result<Duration>;

    /// The focused window.
    ///
    /// `Ok(None)` means nothing is focused, which is routine: it happens whenever focus is on
    /// the desktop, in between window switches, and on the lock screen. `Err` is reserved for
    /// the implementation itself not working - a missing tool, an unreachable display server,
    /// output that could not be understood - and is what turns the tray icon red.
    fn get_active_window(&self) -> Result<Option<Box<dyn Window>>>;

    fn before_main_loop_start(&self) -> Result<()> {
        Ok(())
    }
}
