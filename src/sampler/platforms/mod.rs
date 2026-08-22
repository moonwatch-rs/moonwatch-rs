//! Platform implementations of [`crate::sampler::desktop::Desktop`].

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("Only 'linux' and 'windows' are currently supported");
