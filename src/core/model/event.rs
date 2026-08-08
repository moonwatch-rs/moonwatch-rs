use chrono::{DateTime, TimeDelta, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};

/// Events logged by Moonwatch (on-disk representation)
#[serde_as]
#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(tag = "type")]
pub enum Event {
    /// # ActiveWindowEventV1
    /// An event describing what window was active (desktop only)
    ///
    /// This event is gathered by the desktop app and is based on sampling in regular intervals;
    /// we do not actually know how long each window is in the foreground, but we guess based on
    /// how many times we "catch it" being active and what the sampling interval is.
    ///
    /// Moonwatch samples process path and window name - please see `PreprocessingConfig` definition
    /// for ways to customize what data gets logged and how. As a hard rule, window names are never logged;
    /// they can only be used as a predicate in the preprocessing config.
    ///
    /// Moonwatch does not have any further insight into the active window beyond its title and process path.
    /// This is by design - chiefly to reduce Moonwatch's exposure to your data. As a consequence,
    /// visibility into complex applications - like web browsers - is limited: if you wish to get a break-down
    /// of websites you visit, this is only possible by setting up filters based on the website/window title
    /// or possibly by using different browsers for different purposes.
    ///
    /// For a mobile sibling event, see `ActiveActivityEventV1`.
    ActiveWindowEventV1 {
        /// # Time
        /// Date and time when the event is recorded.
        time: DateTime<Utc>,
        data: ActiveWindowEventV1Data,
    },

    /// # ActiveWindowEvent
    /// Legacy version of `ActiveWindowEventV1`.
    // Note that this uses snake_case for attribute names even in the JSON, unlike the V1 variants.
    #[deprecated]
    ActiveWindowEvent {
        /// # Time
        /// Date and time when the event is recorded.
        time: DateTime<Utc>,

        /// # Duration (in seconds)
        /// Inferred duration for which the window was active - Moonwatch samples at frequent regular
        /// intervals and we assume that the window was active during the whole sampling interval.
        /// In the logs, it will look like the duration for all desktop events is the same - this is normal.
        #[serde_as(as = "DurationSeconds<i64>")]
        #[schemars(with = "i64", range(min = 0))]
        duration: TimeDelta,

        /// # Host name
        /// Name of the computer where the event was sampled.
        hostname: String,

        /// # User name
        /// Name of the user logged into the desktop session where the event was sampled.
        username: String,

        /// # Idle for (in seconds)
        /// Amount of time since last user interaction at `time`. This can be subsequently used
        /// to filter out periods of inactivity from the logs.
        #[serde_as(as = "DurationSeconds<i64>")]
        #[schemars(with = "i64")]
        idle_for: TimeDelta,

        /// # Process path
        /// Absolute path to process binary of the active window. This might be null if
        /// the path was redacted according to Moonwatch config before the log was written.
        process_path: Option<String>,

        /// # Tags
        /// Array of string tags that are user-assigned to the event based on Moonwatch config.
        tags: Vec<String>,
    },

    /// # ActiveActivityEventV1
    /// An event describing that an app was in the foreground (mobile only)
    ///
    /// This event is gathered by the mobile app and is based on OS tracking, which means
    /// that the recorded duration is variable (unlike the equivalent desktop event).
    /// There is a setting in `PostprocessingConfig` to enforce maximum duration for
    /// mobile events, which splits them into shorter segments that are more handy
    /// for resampling.
    ///
    /// For a desktop sibling event, see `ActiveWindowEventV1`.
    ActiveActivityEventV1 {
        /// # Time
        /// Date and time when the event is recorded.
        time: DateTime<Utc>,
        data: ActiveActivityEventV1Data,
    },

    /// # DeviceUnlockEventV1
    /// An event describing that a screen lock was unlocked (mobile only)
    ///
    /// This event is gathered by the mobile app and is based on OS tracking.
    DeviceUnlockEventV1 {
        /// # Time
        /// Date and time when the event is recorded.
        time: DateTime<Utc>,
        data: DeviceUnlockEventV1Data,
    },
}

#[serde_as]
#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(inline)]
pub struct ActiveWindowEventV1Data {
    /// # Duration (in seconds)
    /// Inferred duration for which the window was active - Moonwatch samples at frequent regular
    /// intervals and we assume that the window was active during the whole sampling interval.
    /// In the logs, it will look like the duration for all desktop events is the same - this is normal.
    #[serde_as(as = "DurationSeconds<i64>")]
    #[schemars(with = "i64", range(min = 0))]
    pub duration: TimeDelta,

    /// # Host name
    /// Name of the computer where the event was sampled.
    pub hostname: String,

    /// # User name
    /// Name of the user logged into the desktop session where the event was sampled.
    pub username: String,

    /// # Idle for (in seconds)
    /// Amount of time since last user interaction at `time`. This can be subsequently used
    /// to filter out periods of inactivity from the logs.
    #[serde_as(as = "DurationSeconds<i64>")]
    #[schemars(with = "i64")]
    pub idle_for: TimeDelta,

    /// # Process path
    /// Absolute path to process binary of the active window. This might be null if
    /// the path was redacted according to Moonwatch config before the log was written.
    pub process_path: Option<String>,

    /// # Tags
    /// Array of string tags that are user-assigned to the event based on Moonwatch config.
    pub tags: Vec<String>,
}


#[serde_as]
#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(inline)]
pub struct ActiveActivityEventV1Data {
    /// # Duration (in seconds)
    /// Duration for which the Android app window (activity) was in the foreground.
    /// Since this is based on OS tracking, it is a precise value with no particular upper bound.
    #[serde_as(as = "DurationSeconds<i64>")]
    #[schemars(with = "i64", range(min = 0))]
    pub duration: TimeDelta,

    /// # Host name
    /// Name of the phone where the event was sampled.
    pub hostname: String,

    /// # Application label
    /// Human-readable label of the app to which the activity belongs.
    #[schemars(example = &"Firefox")]
    pub application_label: String,

    /// # Application ID
    /// Dot-separated ID of the app to which the activity belongs.
    #[schemars(example = &"org.mozilla.firefox")]
    pub application_id: String,
}

#[derive(PartialEq, Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(inline)]
pub struct DeviceUnlockEventV1Data {
    /// # Host name
    /// Name of the phone where the event was sampled.
    pub hostname: String,
}

impl Event {
    #[allow(deprecated)]
    pub fn migrate_to_latest(self) -> Event {
        match self {
            Event::ActiveWindowEvent {
                time,
                duration,
                hostname,
                username,
                idle_for,
                process_path,
                tags
            } => Event::ActiveWindowEventV1 {
                time,
                data: ActiveWindowEventV1Data {
                    duration,
                    hostname,
                    username,
                    idle_for,
                    process_path,
                    tags,
                }
            },
            _ => self,
        }
    }
}
