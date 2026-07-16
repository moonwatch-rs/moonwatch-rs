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
  - tested on Ubuntu 22.04 LTS, Ubuntu 24.04 LTS
- Windows
  - no dependencies
  - tested on Windows 10 22H2, Windows 11

### Installation

Tested on Ubuntu 24.04 LTS.

- `sudo apt install gnome-screensaver xprintidle xdotool`
- Clone the repository.
- `./build_linux.py && ./build/moonwatch-rs_0.1.0_Linux-x86-64/install_unix.py`
- This will install into `~/.moonwatcher-rs`.
  - It sets up a Systemd user service `moonwatcher-rs` that starts `moonwatcher` on startup.
  - Events are written to `~/.moonwatcher-rs/logs`
  - To customize, edit `~/.moonwatcher-rs/config.json`
  - To check up on the daemon, run `systemctl --user status moonwatch-rs`
  - To reload config, run `systemctl --user reload moonwatch-rs`

### CLI

```sh
moonwatcher config.json
```

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
