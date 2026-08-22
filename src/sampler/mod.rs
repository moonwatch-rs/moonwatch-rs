//! This module contains functions for capturing events.
//!
//! [`desktop`] defines what Moonwatch needs from the operating system, [`platforms`] holds
//! the per-platform implementations, and [`sampler`] turns one query of the desktop into a
//! [`model::event::RuntimeEvent`] for the recorder to process.

pub mod model;
pub mod desktop;
pub mod platforms;
pub mod sampler;

use anyhow::Result;
use crate::sampler::desktop::Desktop;

/// The desktop implementation to use on this machine.
///
/// Fails when the implementation is not usable at all (a missing helper program, say), which
/// the caller reports rather than treating as a reason to stop.
pub fn get_desktop() -> Result<Box<dyn Desktop>> {
    #[cfg(unix)]
    fn get_desktop_impl() -> Result<Box<dyn Desktop>> {
        // TODO support more UNIX platforms, possibly use config to request a particular impl.

        let desktop = Box::new(platforms::linux::GnomeDesktop);

        desktop.check_implementation_available()?;
        Ok(desktop)
    }

    #[cfg(windows)]
    fn get_desktop_impl() -> Result<Box<dyn Desktop>> {
        let desktop = Box::new(platforms::windows::WindowsDesktop);
        desktop.check_implementation_available()?;
        Ok(desktop)
    }

    get_desktop_impl()
}
