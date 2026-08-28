[![Moonwatch.rs build master branch](https://img.shields.io/github/actions/workflow/status/moonwatch-rs/moonwatch-rs/ci.yml?branch=master)](https://github.com/moonwatch-rs/moonwatch-rs/actions)

# Moonwatch.rs

🚧 _This is a personal project that's been running for a few years.
It's useful, but expect roughness around the edges._ 🚧

Moonwatch.rs is a privacy-focused digital wellbeing app. Get insights into how you
spend your screen time – you choose what data is tracked and where it is stored.

You can run Moonwatch.rs completely self-hosted on your Windows or Linux
machine (for mobile, see [moonwatch-android](https://github.com/moonwatch-rs/moonwatch-android)).
It is a decentralized system; each instance logs events independently and each
can process the logs for further analysis.

If you want to aggregate data from multiple devices, just make sure the logs
end up in a single location (using Syncthing, a network drive, a cloud service
like Dropbox, OneDrive, MEGA, etc.).

## Supported platforms

- Windows
- Linux (and other unix-like systems), GNOME, X11
  - dependencies: `gnome-screensaver-command`, `xprintidle`, `xdotool`, GTK 3, libappindicator
- Android
  - see [moonwatch-android](https://github.com/moonwatch-rs/moonwatch-android)

## Installation and setup

There is a single binary, `moonwatch_rs`, which can install itself into `~/.moonwatch-rs`
and set up autostart when you log in (using a Windows Run key or Systemd service), all
by running the command:

```
moonwatch_rs install
```

After installation, a tray icon will appear in your desktop. Use it to navigate into
the `~/.moonwatch-rs`. There are multiple JSON configuration files which are best edited
using VS Code which will give you suggestions and validation through the provided JSON schemas.

## Architecture

(Your Desktop) → **Sampler** → **Recorder** → (events written to disk as JSON) → **Pipeline** → (Parquet/CSV bundle)

There is a background service running the `moonwatch_rs watch` command, which periodically polls
your desktop and logs events to disk. This service also has a tray icon which you can use
to interact with it, see if it's running, and manually trigger an ETL-like pipeline that
processes all the logs into one big bundle.

### Sampler and Recorder

Sampler is the component that periodically gathers information from your OS and produces events
that the Recorder does some user-defined processing on before eventually flushing them onto disk.

At this time, there are these events:

- `ActiveWindowEventV1` (desktop only) - information about the active window on your desktop,
  including its process and window title (this is only used for Recorder rules, never written to disk)
- `ActiveActivityEventV1` (mobile only) - information about the active Android activity
- `DeviceUnlockEventV1` (mobile only) - information about Android device unlock

The desktop app has configurable Recorder using the config `recorder_config.json`. Here is an
example of what you can do:

- redact events
- tag events based on their properties (this is mostly useful to account for `windowTitle`
  which is not available in later processing stages)

```json
{
  "$schema": "./schemas/recorder_config.schema.json",
  "activeWindowEventRules": [
    {
      "predicate": {
        "attributeRegex": {
          "name": "processPath",
          "regex": "Microsoft\\.LockApp|Windows\\\\explorer.exe"
        }
      },
      "actions": ["delete"]
    },
    {
      "predicate": {
        "attributeRegex": {
          "name": "processName",
          "regex": "firefox|chrome|brave"
        }
      },
      "actions": [{"addTag": "browser"}]
    },
    {
      "predicate": {
        "and": [
          {"hasTag": "browser"},
          {
            "attributeRegex": {
              "name": "windowTitle",
              "regex": "YouTube [-–—] (Mozilla Firefox|Google Chrome|Brave)$"
            }
          }
        ]
      },
      "actions": [{"addTag": "youtube"}]
    }
  ]
}
```

### Event log storage

This is preferably pointed into some "shared folder" in `main_config.json` (it is useful to put your
recorder and pipeline configs into a shared location, too). Here, the Recorder will periodically write
a JSONL file (one JSON object per line) with all the events, using a UUIDv7 filename so that it is unique.

Event log compaction into `.jsonl.gz` is also supported, though Moonwatch.rs currently does not provide a way to produce these files;
however, Pipeline can ingest them.

### ETL-like pipeline

Pipeline is what takes the sum of all your desktop and mobile logs, unifies them into one `ActiveEvent`
structure, applies another set of rules similar to what Recorder does, and finally writes it all out
as a `.parquet` or `.csv` file. Currently, this is where the pipeline ends, though a Grafana setup
may be provided in the future.

The desktop app has configurable Pipeline using the config `pipeline_config.json`.
It can be triggered from the tray icon menu or by running `moonwatch_rs pipeline`.
Here is an example of what you can do:

- assign categories (events can have multiple tags but only a single category)
- remove periods of idle activity
- split long events into multiple smaller ones for easier aggregation (currently not implemented)

```json
{
  "$schema": "./schemas/pipeline_config.schema.json",
  "activeEventRules": [
    {
      "predicate": {
        "attributeRegex": {
          "name": "name",
          "regex": "vlc|mpc-hc64"
        }
      },
      "actions": [
        {"addTag": "video"},
        {"setAttribute": {"name": "category", "value": "video"}}]
    },
    {
      "predicate": {
        "and": [
          {"idleForGreaterThanSec": 300},
          {
            "not": {
              "or": [
                {"hasTag": "youtube"},
                {"hasTag": "video"}]}}]
      },
      "actions": ["ignore"]
    },
    {
      "predicate": {
        "attributeRegex": {
          "name": "processPath",
          "regex": "[\\\\/](steamapps|GOG Galaxy|GOGLibrary|Games)[\\\\/]"
        }
      },
      "actions": [
        {
          "setAttribute": {
            "name": "category",
            "value": "gaming"
          }}]}
  ],
  "activeEventMaxDuration": 600
}
```
