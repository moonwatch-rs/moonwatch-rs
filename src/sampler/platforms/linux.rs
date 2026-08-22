//! Active window sampling on X11 desktops, by shelling out to `xdotool`, `xprintidle`
//! and `gnome-screensaver-command`.

use std::process::{Command, Stdio};
use std::time::Duration;
use std::fs;
use std::path::PathBuf;
use crate::sampler::desktop::{Desktop, Window};
use anyhow::{anyhow, bail, Context, Result};

pub struct GnomeDesktop;
pub struct LinuxXWindow { window_id: u64 }

/// A command that ran but reported failure.
struct CommandFailure {
    program: &'static str,
    status: std::process::ExitStatus,
    stderr: String,
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed ({})", self.program, self.status)?;
        if !self.stderr.is_empty() {
            write!(f, ": {}", self.stderr)?;
        }
        Ok(())
    }
}

impl CommandFailure {
    /// Whether this looks like the X server being unreachable rather than the command simply
    /// having nothing to report.
    ///
    /// This is what a Wayland session looks like from here: `xdotool -h` succeeds, so
    /// [`GnomeDesktop::check_implementation_available`] is happy, and then every real call
    /// fails like this. Matching on stderr is a heuristic, so it is deliberately fail-safe -
    /// anything unrecognised counts as "nothing to report", which stays quiet.
    fn is_display_unreachable(&self) -> bool {
        let stderr = self.stderr.to_ascii_lowercase();
        ["can't open display", "cannot open display", "unable to open display",
         "no protocol specified", "bad display name"]
            .iter()
            .any(|symptom| stderr.contains(symptom))
    }
}

/// Why running a command produced no output.
enum CommandError {
    /// It ran and reported failure. Whether that is a malfunction depends on the command:
    /// `xdotool getactivewindow` exits non-zero both when nothing is focused (routine) and
    /// when X is unreachable (not), so only the caller can decide.
    Failed(CommandFailure),
    /// It could not be run at all - removed, not executable. Always a malfunction.
    NotRun(anyhow::Error),
}

/// Run `program` and return its trimmed standard output.
fn run(program: &'static str, args: &[&str]) -> Result<String, CommandError> {
    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| CommandError::NotRun(
            anyhow::Error::new(e).context(format!("could not run {program}"))))?;

    if !output.status.success() {
        return Err(CommandError::Failed(CommandFailure {
            program,
            status: output.status,
            // Lossy rather than strict: this text is only ever logged or shown, so mangled
            // bytes must not turn into a different error than the one that happened.
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// [`run`], where any failure at all is a malfunction.
fn run_or_fail(program: &'static str, args: &[&str]) -> Result<String> {
    run(program, args).map_err(|e| match e {
        CommandError::Failed(failure) => anyhow!("{failure}"),
        CommandError::NotRun(e) => e,
    })
}

impl Desktop for GnomeDesktop {
    fn implementation_name(&self) -> &'static str {
        "GnomeDesktop"
    }

    fn check_implementation_available(&self) -> Result<()> {
        let commands_to_test = ["gnome-screensaver-command", "xprintidle", "xdotool"];

        for cmd in commands_to_test {
            let output = Command::new(cmd)
                .arg("-h")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output();

            if let Err(e) = output {
                bail!("Program {cmd:?} not available: {e}")
            }
        }

        Ok(())
    }

    fn is_screen_locked(&self) -> bool {
        // Anything other than a clear "yes" counts as unlocked: gnome-screensaver is absent
        // on modern GNOME, and refusing to sample because of that would be worse than
        // occasionally sampling a locked screen.
        run("gnome-screensaver-command", &["-q"])
            .is_ok_and(|status| status.contains("is active"))
    }

    fn get_idle_duration(&self) -> Result<Duration> {
        let output = run_or_fail("xprintidle", &[])?;

        let idle_ms = output.parse::<u64>()
            .with_context(|| format!("xprintidle printed {output:?}, expected milliseconds"))?;

        Ok(Duration::from_millis(idle_ms))
    }

    fn get_active_window(&self) -> Result<Option<Box<dyn Window>>> {
        let output = match run("xdotool", &["getactivewindow"]) {
            Ok(output) => output,
            // xdotool exits non-zero when nothing is focused, which is routine, and when it
            // cannot reach X, which is not.
            Err(CommandError::Failed(failure)) if failure.is_display_unreachable() => {
                bail!("{failure}")
            }
            Err(CommandError::Failed(failure)) => {
                log::debug!("No active window: {failure}");
                return Ok(None);
            }
            Err(CommandError::NotRun(e)) => return Err(e),
        };

        let window_id = output.parse::<u64>()
            .with_context(|| format!("xdotool printed window id {output:?}"))?;

        Ok(Some(Box::new(LinuxXWindow { window_id })))
    }
}

impl Window for LinuxXWindow {
    fn get_title(&self) -> Result<String> {
        run_or_fail("xdotool", &["getwindowname", &self.window_id.to_string()])
    }

    fn get_process_id(&self) -> Result<u64> {
        let output = run_or_fail("xdotool", &["getwindowpid", &self.window_id.to_string()])?;

        output.parse::<u64>().with_context(|| format!("xdotool printed pid {output:?}"))
    }

    fn get_process_path(&self) -> Result<PathBuf> {
        let pid = self.get_process_id()?;
        let exe = format!("/proc/{pid}/exe");

        fs::read_link(&exe).with_context(|| format!("could not read {exe}"))
    }
}
