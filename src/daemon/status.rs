//! What the daemon is currently doing, as shown by the tray icon and its menu.
//!
//! The worker thread owns the truth about this (it is the one that loads configuration and
//! samples windows) but the UI thread is the only one allowed to touch the tray, so the
//! state lives in a [`SharedStatus`] that the worker writes and the tray reads.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Longest tooltip Windows will display for a notification icon: `NOTIFYICONDATAW::szTip`
/// holds 128 UTF-16 units including the terminator.
const MAX_TOOLTIP_CHARS: usize = 127;

/// How much of a problem description to show in the menu. The full message always goes to
/// `moonwatch_rs.log`.
const MAX_PROBLEM_CHARS: usize = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingState {
    Recording,
    Paused,
    /// Nothing is being sampled, because no usable configuration has been loaded.
    #[default]
    Stopped,
}

/// Which of the three embedded tray icons to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIcon {
    Recording,
    Paused,
    Problem,
}

#[derive(Debug, Clone, Default)]
pub struct MoonwatcherStatus {
    pub recording: RecordingState,
    /// Why the configuration is unusable, phrased for the user. Set when the configuration
    /// file could not be read and when no desktop implementation could be initialised.
    pub config_problem: Option<String>,
    /// Why sampling the active window is not working. Only ever set for a genuine
    /// malfunction - a missing tool, an unreachable display server - never for the routine
    /// case of nothing being focused.
    pub sampling_problem: Option<String>,
    /// Sampling interval of the loaded configuration; `None` until one loads.
    pub sample_every: Option<Duration>,
}

impl MoonwatcherStatus {
    /// A problem outranks everything else: the whole point of the error icon is that a
    /// configuration the user has just broken should be obvious, even though a failed
    /// *reload* leaves the previous configuration recording happily.
    pub fn icon(&self) -> StatusIcon {
        if self.config_problem.is_some()
            || self.sampling_problem.is_some()
            || self.recording == RecordingState::Stopped {
            StatusIcon::Problem
        } else if self.recording == RecordingState::Paused {
            StatusIcon::Paused
        } else {
            StatusIcon::Recording
        }
    }

    /// Text for the disabled item at the top of the tray menu. This is the only textual
    /// channel on Linux, where the appindicator backend ignores tooltips altogether.
    ///
    /// A configuration problem is shown ahead of a sampling problem: it is the more
    /// actionable of the two, and a configuration that will not load tends to be the cause
    /// of whatever else is wrong.
    pub fn menu_line(&self) -> String {
        if let Some(problem) = &self.config_problem {
            let problem = shorten(problem, MAX_PROBLEM_CHARS);
            return match self.recording {
                RecordingState::Stopped => format!("Not recording - {problem}"),
                _ => format!("{problem} - previous settings still in use"),
            };
        }

        if let Some(problem) = &self.sampling_problem {
            return format!("Sampling failed - {}", shorten(problem, MAX_PROBLEM_CHARS));
        }

        match self.recording {
            RecordingState::Recording => match self.sample_every {
                Some(every) => format!("Recording every {} s", every.as_secs()),
                None => "Recording".to_string(),
            },
            RecordingState::Paused => "Recording paused".to_string(),
            RecordingState::Stopped => "Not recording".to_string(),
        }
    }

    /// Windows tray tooltip. Ignored on Linux, see [`MoonwatcherStatus::menu_line`].
    pub fn tooltip(&self) -> String {
        shorten(&format!("Moonwatch.rs - {}", self.menu_line()), MAX_TOOLTIP_CHARS)
    }
}

/// Flatten to a single line and clip to `max_chars`.
///
/// Problem descriptions come from `anyhow`, so they can be several lines long; neither a
/// menu item nor a tooltip renders that usefully.
fn shorten(text: &str, max_chars: usize) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if single_line.chars().count() <= max_chars {
        return single_line;
    }

    let mut clipped: String = single_line.chars().take(max_chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

/// The current status, shared between the worker thread and the tray.
///
/// Every mutation repaints the tray, so there is no way to change the state and forget to
/// tell the user about it.
#[derive(Clone, Default)]
pub struct SharedStatus(Arc<Mutex<MoonwatcherStatus>>);

impl SharedStatus {
    pub fn new() -> SharedStatus {
        SharedStatus::default()
    }

    pub fn get(&self) -> MoonwatcherStatus {
        self.lock().clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut MoonwatcherStatus)) {
        change(&mut self.lock());
        // Released the lock first: the UI thread is about to read it.
        super::request_ui_refresh();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MoonwatcherStatus> {
        // A panic while holding this lock would only have left the displayed state stale,
        // which is not worth propagating the poison for.
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> MoonwatcherStatus {
        MoonwatcherStatus {
            recording: RecordingState::Recording,
            config_problem: None,
            sampling_problem: None,
            sample_every: Some(Duration::from_secs(15)),
        }
    }

    #[test]
    fn healthy_states_map_to_their_own_icon_and_text() {
        let status = recording();
        assert_eq!(status.icon(), StatusIcon::Recording);
        assert_eq!(status.menu_line(), "Recording every 15 s");
        assert_eq!(status.tooltip(), "Moonwatch.rs - Recording every 15 s");

        let paused = MoonwatcherStatus { recording: RecordingState::Paused, ..recording() };
        assert_eq!(paused.icon(), StatusIcon::Paused);
        assert_eq!(paused.menu_line(), "Recording paused");
    }

    #[test]
    fn a_problem_outranks_recording_and_paused() {
        let problem = Some("config.json is broken".to_string());

        let while_recording = MoonwatcherStatus { config_problem: problem.clone(), ..recording() };
        assert_eq!(while_recording.icon(), StatusIcon::Problem);
        assert_eq!(while_recording.menu_line(),
                   "config.json is broken - previous settings still in use");

        let while_paused = MoonwatcherStatus {
            recording: RecordingState::Paused,
            config_problem: problem.clone(),
            ..recording()
        };
        assert_eq!(while_paused.icon(), StatusIcon::Problem);
    }

    /// A failing xdotool has to be as visible as a broken config.json.
    #[test]
    fn a_sampling_problem_shows_the_error_icon_and_says_what_broke() {
        let status = MoonwatcherStatus {
            sampling_problem: Some("xdotool failed (exit status: 1): Can't open display".to_string()),
            ..recording()
        };

        assert_eq!(status.icon(), StatusIcon::Problem);
        assert_eq!(status.menu_line(),
                   "Sampling failed - xdotool failed (exit status: 1): Can't open display");
        assert!(status.tooltip().starts_with("Moonwatch.rs - Sampling failed - "));
    }

    /// With both broken, the configuration is the one worth telling the user about: it is
    /// what they can act on, and it is often why sampling stopped working too.
    #[test]
    fn a_configuration_problem_is_shown_ahead_of_a_sampling_problem() {
        let status = MoonwatcherStatus {
            config_problem: Some("config.json could not be loaded".to_string()),
            sampling_problem: Some("xdotool failed".to_string()),
            ..recording()
        };

        assert_eq!(status.icon(), StatusIcon::Problem);
        assert_eq!(status.menu_line(),
                   "config.json could not be loaded - previous settings still in use");
    }

    #[test]
    fn nothing_loaded_reports_not_recording() {
        // What the daemon looks like when the configuration was already broken at login.
        let stopped = MoonwatcherStatus {
            recording: RecordingState::Stopped,
            config_problem: Some("no such file".to_string()),
            sampling_problem: None,
            sample_every: None,
        };
        assert_eq!(stopped.icon(), StatusIcon::Problem);
        assert_eq!(stopped.menu_line(), "Not recording - no such file");

        // Defensively: stopped without a stated reason is still not healthy.
        let default = MoonwatcherStatus::default();
        assert_eq!(default.icon(), StatusIcon::Problem);
        assert_eq!(default.menu_line(), "Not recording");
    }

    #[test]
    fn multiline_problems_are_flattened_and_clipped() {
        let status = MoonwatcherStatus {
            config_problem: Some("Failed to reload configuration\n\nCaused by:\n    expected `,` or `}` at line 12 column 3".to_string()),
            ..recording()
        };

        let line = status.menu_line();
        assert!(!line.contains('\n'), "menu items cannot show newlines: {line:?}");
        assert!(line.starts_with("Failed to reload configuration Caused by: expected"),
                "got {line:?}");
        assert!(line.ends_with("previous settings still in use"));
    }

    #[test]
    fn tooltip_fits_what_windows_will_display() {
        let status = MoonwatcherStatus {
            config_problem: Some("x".repeat(500)),
            ..recording()
        };

        let tooltip = status.tooltip();
        assert!(tooltip.chars().count() <= MAX_TOOLTIP_CHARS,
                "tooltip was {} chars", tooltip.chars().count());
        assert!(tooltip.ends_with('…'), "clipped text should say so: {tooltip:?}");
    }
}
