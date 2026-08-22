//! One sample of the desktop, classified.
//!
//! The distinction this module exists for is *routine outcome* versus *malfunction*. A
//! locked screen or nothing being focused happens constantly and must stay quiet; a desktop
//! implementation that has stopped working is what the user needs to be told about, and is
//! the only case that becomes an `Err`.

use std::time::Duration;
use anyhow::Result;
use log::debug;
use crate::sampler::desktop::Desktop;
use crate::sampler::model::event::{RuntimeActiveWindowEvent, RuntimeEvent};

/// The outcome of one sample.
///
/// Everything here is a normal outcome; an `Err` from [`sample_active_window`] means the
/// desktop implementation is not working, and is the only case reported to the user.
#[derive(Debug)]
pub enum SampleOutcome {
    /// The screen is locked, so there is nothing to record.
    ScreenLocked,
    /// Nothing is focused. Routine: focus on the desktop, a window being switched, and so on.
    NoActiveWindow,
    /// A window was active and an event was produced for it.
    Event(RuntimeEvent),
}

/// Query the desktop once and build an event out of what it reports.
///
/// `duration` is the sampling interval, which is credited to the window that was caught.
pub fn sample_active_window(desktop: &dyn Desktop, duration: Duration) -> Result<SampleOutcome> {
    if desktop.is_screen_locked() {
        return Ok(SampleOutcome::ScreenLocked);
    }

    let Some(window) = desktop.get_active_window()? else {
        return Ok(SampleOutcome::NoActiveWindow);
    };

    let idle_duration = desktop.get_idle_duration()?;
    let window_title = window.get_title().unwrap_or_default();

    // Not being able to read the path is expected for elevated processes, processes owned by
    // another user, and processes that have just exited. We still know the user was active,
    // so record the event without it rather than losing the sample - and do not treat it as
    // the implementation being broken.
    let process_path = match window.get_process_path() {
        Ok(path) => Some(path),
        Err(e) => {
            debug!("Could not determine the process path: {e:#}");
            None
        }
    };

    let event = RuntimeActiveWindowEvent::new(idle_duration, window_title, process_path, duration);
    Ok(SampleOutcome::Event(event.into()))
}

/// What a sample outcome means for the tray: `Some` only for a genuine malfunction.
///
/// Routine outcomes - a locked screen, nothing focused - must never turn the icon red, or it
/// would be red most of the time for no reason.
pub fn sampling_problem(result: &Result<SampleOutcome>) -> Option<String> {
    match result {
        Ok(_) => None,
        Err(e) => Some(format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use anyhow::{bail, Context};
    use chrono::TimeDelta;
    use crate::sampler::desktop::Window;

    /// A [`Desktop`] whose every outcome can be dictated, so the routine-versus-malfunction
    /// classification can be tested without a real desktop on either platform.
    #[derive(Default)]
    struct FakeDesktop {
        locked: bool,
        /// `Ok(None)` is nothing focused, `Err` is a broken implementation.
        active_window: Option<FakeWindow>,
        window_lookup_fails: bool,
        idle_fails: bool,
    }

    #[derive(Clone, Default)]
    struct FakeWindow {
        title: String,
        process_path: Option<PathBuf>,
    }

    impl Desktop for FakeDesktop {
        fn implementation_name(&self) -> &'static str { "FakeDesktop" }

        fn is_screen_locked(&self) -> bool { self.locked }

        fn get_idle_duration(&self) -> Result<Duration> {
            if self.idle_fails {
                bail!("xprintidle failed (exit status: 1): cannot open display");
            }
            Ok(Duration::from_secs(5))
        }

        fn get_active_window(&self) -> Result<Option<Box<dyn Window>>> {
            if self.window_lookup_fails {
                bail!("xdotool failed (exit status: 1): cannot open display");
            }
            Ok(self.active_window.clone().map(|window| Box::new(window) as Box<dyn Window>))
        }
    }

    impl Window for FakeWindow {
        fn get_title(&self) -> Result<String> { Ok(self.title.clone()) }

        fn get_process_id(&self) -> Result<u64> { Ok(1234) }

        fn get_process_path(&self) -> Result<PathBuf> {
            self.process_path.clone().context("Access is denied. (0x80070005)")
        }
    }

    fn sample(desktop: &FakeDesktop) -> Result<SampleOutcome> {
        sample_active_window(desktop, Duration::from_secs(15))
    }

    fn active_window_event(result: Result<SampleOutcome>) -> RuntimeActiveWindowEvent {
        match result {
            Ok(SampleOutcome::Event(RuntimeEvent::ActiveWindowEvent(e))) => e,
            other => panic!("expected an active window event, got {other:?}"),
        }
    }

    #[test]
    fn nothing_focused_is_routine_and_not_reported() {
        let result = sample(&FakeDesktop { active_window: None, ..Default::default() });

        assert!(matches!(result, Ok(SampleOutcome::NoActiveWindow)), "got {result:?}");
        assert_eq!(sampling_problem(&result), None, "the tray must stay calm");
    }

    #[test]
    fn a_locked_screen_is_routine_and_not_reported() {
        let result = sample(&FakeDesktop { locked: true, ..Default::default() });

        assert!(matches!(result, Ok(SampleOutcome::ScreenLocked)), "got {result:?}");
        assert_eq!(sampling_problem(&result), None);
    }

    /// The case this classification exists for: xdotool (or its equivalent) stops working.
    #[test]
    fn a_broken_implementation_is_reported_to_the_tray() {
        let result = sample(&FakeDesktop { window_lookup_fails: true, ..Default::default() });

        let problem = sampling_problem(&result).expect("a malfunction must be reported");
        assert!(problem.contains("cannot open display"), "got {problem:?}");
    }

    /// Idle time is part of gathering an event, so failing to read it is a malfunction too -
    /// reporting it as zero idle time would be silently wrong data.
    #[test]
    fn failing_to_read_idle_time_is_reported() {
        let result = sample(&FakeDesktop {
            active_window: Some(FakeWindow::default()),
            idle_fails: true,
            ..Default::default()
        });

        let problem = sampling_problem(&result).expect("a malfunction must be reported");
        assert!(problem.contains("xprintidle"), "got {problem:?}");
    }

    /// An elevated window on Windows: the path cannot be read, but the sample is still good.
    #[test]
    fn an_unreadable_process_path_still_records_the_event() {
        let result = sample(&FakeDesktop {
            active_window: Some(FakeWindow {
                title: "Task Manager".to_string(),
                process_path: None,
            }),
            ..Default::default()
        });

        assert_eq!(sampling_problem(&result), None, "this is not a malfunction");
        let e = active_window_event(result);
        assert_eq!(e.data.process_path, None);
        assert_eq!(e.window_title, "Task Manager");
    }

    #[test]
    fn a_normal_sample_produces_an_event_with_its_path() {
        let result = sample(&FakeDesktop {
            active_window: Some(FakeWindow {
                title: "Some Window".to_string(),
                process_path: Some(PathBuf::from("/usr/bin/firefox")),
            }),
            ..Default::default()
        });

        assert_eq!(sampling_problem(&result), None);
        let e = active_window_event(result);
        assert_eq!(e.data.process_path.as_deref(), Some("/usr/bin/firefox"));
        assert_eq!(e.data.idle_for, TimeDelta::seconds(5));
        assert_eq!(e.data.duration, TimeDelta::seconds(15));
        assert!(e.data.tags.is_empty(), "tagging is the recorder's job");
        assert!(!e.data.hostname.is_empty());
    }
}
