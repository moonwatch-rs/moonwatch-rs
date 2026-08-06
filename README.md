# Moonwatch.rs

[![Moonwatch.rs build master branch](https://img.shields.io/github/actions/workflow/status/tkarabela/moonwatch-rs/ci.yml?branch=master)](https://github.com/tkarabela/moonwatch-rs/actions)

🚧 _This is an early development version of the software._ 🚧

Moonwatch.rs is a privacy-focused digital wellbeing app. Get insights into how you
spend your screen time – you choose what data is tracked and where it is stored.

You can run Moonwatch.rs completely self-hosted on your desktop or laptop;
aggregating data from multiple machines is also possible via a network drive or
any of the "Shared Folder" cloud services (eg. Dropbox, OneDrive, MEGA, etc.).

_Currently, Moonwatch.rs consists only of the `moonwatcher` daemon, which is a 
background service recording active window at regular intervals and logging it
into `.jsonl` files. More features including analytics and GUI are planned._

## The `moonwatcher` daemon

### Supported platforms

- Linux (and other unix-like systems), GNOME, X11
  - dependencies: `gnome-screensaver-command`, `xprintidle`, `xdotool`
  - for the tray icon: GTK 3 and libappindicator (see [Tray icon](#tray-icon))
  - tested on Ubuntu 22.04 LTS, Ubuntu 24.04 LTS
- Windows
  - no dependencies
  - tested on Windows 10 22H2, Windows 11

### Tray icon

While running, `moonwatcher` shows a moon icon in the system tray. The icon tells you
whether it is actually recording:

| icon | meaning |
|---|---|
| amber moon | configuration loaded, recording events |
| grey moon | configuration loaded, recording paused |
| grey moon with a red dot | something is wrong – see the menu for what |

The first line of the context menu spells the same thing out in words, including the
reason when there is a problem, for example
`config.json could not be loaded: expected ',' or '}' at line 12 column 3 - previous
settings still in use`. On Windows the tray tooltip says the same; on Linux it does not,
because the AppIndicator backend has no tooltip support — read the menu instead.

Two kinds of thing turn the icon red. One is `config.json` (see below). The other is the
event gathering itself failing — `xdotool` or `xprintidle` no longer working, for instance,
which is what a Wayland session looks like from here: `xdotool -h` succeeds at startup and
then every real call fails. The menu then reads `Sampling failed - <what went wrong>`, and
the icon clears itself as soon as a sample succeeds.

Failures that are *routine* deliberately do **not** turn the icon red, or it would be red
most of the time: nothing being focused (`xdotool getactivewindow` fails whenever focus is on
the desktop), a locked screen, and not being able to read a process path. That last one
happens on Windows for any elevated window — Task Manager, an admin editor — and such events
are still recorded, just with `processPath: null`, rather than being dropped.

The rest of the menu offers:

- **Reload configuration** – re-read `config.json` (same as `SIGHUP` on Linux)
- **Write events now** – flush buffered events to a `.jsonl` file immediately
- **Pause recording** – stop sampling without stopping the daemon
- **Open log folder** – open the configured `output_dir` (disabled until one is known)
- **Open Moonwatch.rs folder** – open the directory holding `config.json` and the log
- **Quit Moonwatch.rs** – write buffered events and exit

So a `config.json` you have broken is a self-service fix: the icon turns red, the menu says
what the syntax error was, **Open Moonwatch.rs folder** takes you to the file, and
**Reload configuration** picks up your correction. A configuration that fails to load does
**not** stop the daemon — it keeps running (recording with the previous settings if it has
any) so that the tray is still there to fix it from. On Linux that means
`systemctl --user status moonwatch-rs` reports a healthy unit even when the configuration is
broken; the icon and the log are what tell you otherwise.

The daemon keeps working if no tray icon can be created (no display, missing libraries);
it just logs a warning. Pass `--no-tray` to skip it deliberately — on Windows this only
hides the icon, the clean-shutdown handling below stays active.

On **Windows 11** new tray icons start out in the overflow flyout behind the `^` button;
drag the moon onto the taskbar to keep it visible.

On **Linux** the tray icon needs GTK 3 and an AppIndicator implementation
(`sudo apt install libgtk-3-0 libayatana-appindicator3-1`; to build, the matching `-dev`
packages). Note that **GNOME Shell has no built-in tray**: the icon is only displayed if
the "AppIndicator and KStatusNotifierItem Support" extension is installed, which Ubuntu
ships by default but stock Fedora/Debian GNOME does not.

### Shutting down cleanly

Recorded events are buffered in memory and only written out every `write_every_sec`, so
stopping the daemon abruptly loses whatever has not been written yet.

- On Linux, `SIGTERM` (ie. `systemctl --user stop`) triggers a final write.
- On Windows, `moonwatcher` now owns a hidden window and answers `WM_QUERYENDSESSION` /
  `WM_ENDSESSION`, so logging off, restarting and shutting down all flush the buffer
  first. Previously the process had neither a console nor a window and got no notice at
  all, which meant those events were lost.
- Either way, **Quit Moonwatch.rs** in the tray menu writes everything out before exiting.

Killing the process outright (`Stop-Process -Force`, `kill -9`) still loses the buffer;
use **Write events now** or lower `write_every_sec` if that matters to you.

### Diagnostics

`moonwatcher` writes a log file next to `config.json`, `moonwatcher_rCURRENT.log`
(rotated at 2 MB, three files kept). Set `MOONWATCH_LOG=debug` for per-sample detail.
On Linux the log also goes to the systemd journal (`journalctl --user -u moonwatch-rs`).

Every change of state is logged as a `Tray status:` line, so what the icon was showing at
any point can be reconstructed afterwards. Configuration errors appear there in full, rather
than clipped to fit a menu item.

### Installation on Linux

Tested on Ubuntu 24.04 LTS.

- `sudo apt install gnome-screensaver xprintidle xdotool libgtk-3-0 libayatana-appindicator3-1`
- Clone the repository.
- `./build_linux.py && ./build/moonwatch-rs_0.1.0_Linux-x86-64/install_unix.py`
- This will install into `~/.moonwatcher-rs`.
  - It sets up a Systemd user service `moonwatcher-rs` that starts `moonwatcher` on startup.
  - Events are written to `~/.moonwatcher-rs/logs`
  - To customize, edit `~/.moonwatcher-rs/config.json`
  - To check up on the daemon, run `systemctl --user status moonwatch-rs`
  - To reload config, run `systemctl --user reload moonwatch-rs`
  - To stop it, run `systemctl --user stop moonwatch-rs` (this flushes buffered events)

To build from source you additionally need `libgtk-3-dev` and
`libayatana-appindicator3-dev`.

### Installation on Windows

- Build with `build_windows.py` (cross-compiles from Linux) or `cargo build --release`.
- Run `install_windows.bat` from the resulting package.
- This will install into `%USERPROFILE%\.moonwatch-rs`.
  - It adds a shortcut to the Startup folder so `moonwatcher` starts at login.
  - Events are written to `%USERPROFILE%\.moonwatch-rs\log`
  - To customize, edit `%USERPROFILE%\.moonwatch-rs\config.json` (reachable via **Open
    Moonwatch.rs folder** in the tray menu), then use **Reload configuration**
  - To stop it, use **Quit Moonwatch.rs** in the tray menu
  - Buffered events are written out automatically when you log off, restart or shut down

### CLI

```sh
moonwatcher config.json
```

- `--no-tray` – do not create a tray icon

### JSON configuration

The overall structure is as follows (relative paths are taken to start in the directory where the JSON config is located):

- `"main"` (object)
  - `"output_dir"` (string)
    - path to directory where event logs are stored
  - `"sample_every_sec"` (number)
    - delay between sampling (seconds)
  - `"write_every_sec"` (number)
    - delay between writing samples to a file (seconds)
  - `"path_to_base_config"` (string or null)
    - path to another .json configuration file from which "ignore", "anonymize" and "tags" definitions will be read and added to definitions in this config file
    - this is useful for sharing settings across different systems
- `"ignore"` (object, array or null)
  - one or more `WindowEventMatcher` objects (see below)
  - events that match will not be recorded at all
- `"anonymize"` (object, array or null)
  - one or more `WindowEventMatcher` objects (see below)
  - events that match will be recorded in redacted from
- `"tags"` (object)
  - `"<tag name>"` (object, array or null)
    - one or more `WindowEventMatcher` objects (see below)
    - events that match will get assigned `"<tag name>"` in output

A `WindowEventMatcher` definition is an object with at least one of the following keys:

- `"window_title"` (string)
  - a regular expression (`regex::Regex`) that is tested against window title
- `"process_path"` (string)
  - a regular expression (`regex::Regex`) that is tested against process path

The `WindowEventMatcher` definition is used to match events – an event must match
all predicates defined by given `WindowEventMatcher` (AND semantics). If you want
OR semantics, just define multiple `WindowEventMatcher`s.

Full configuration example:

```json
{
  "main": {
    "output_dir": "./logs",
    "sample_every_sec": 15,
    "write_every_sec": 21600,
    "path_to_base_config": null
  },
  "ignore": [{
    "window_title": "title to ignore"
  }],
  "anonymize": [{
    "window_title": "title to anonymize"
  }],
  "tags": {
    "youtube": [{
        "window_title": "YouTube — Mozilla Firefox$",
        "process_name": "firefox(\\.exe)?$"
      },
      {
        "window_title": "YouTube — Mozilla Firefox$",
        "process_name": "chrome(\\.exe)?$"
      }
    ],
    "pycharm": {
      "process_path": "JetBrains/Toolbox/apps/PyCharm"
    },
    "clion": {
      "process_path": "JetBrains/Toolbox/apps/CLion"
    }
  }
}
```
