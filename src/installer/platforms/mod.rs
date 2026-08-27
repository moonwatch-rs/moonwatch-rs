//! Per-platform autostart registration and stopping of a running daemon.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;
