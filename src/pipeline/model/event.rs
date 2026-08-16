use chrono::{DateTime, TimeDelta, Utc};
use schemars::JsonSchema;
use serde_derive::{Deserialize, Serialize};
use crate::core::model::event::DeviceUnlockEventV1Data;

/// Events logged by Moonwatch (ETL pipeline)
/// Note that this is only here to formally denote what attributes are used;
/// for processing, we use a Polars dataframe with equivalent schema.
#[derive(PartialEq, Debug, Clone)]
pub enum PipelineEvent {
    ActiveEvent(ActiveEvent),
    DeviceUnlockEvent {
        time: DateTime<Utc>,
        data: DeviceUnlockEventV1Data,
    },
}

/// An ETL pipeline unification of `ActiveWindowEventV1` and `ActiveActivityEventV1`.
/// Note that this is only here to formally denote what attributes are used;
/// for processing, we use a Polars dataframe with equivalent schema.
#[derive(PartialEq, Debug, Clone)]
pub struct ActiveEvent {
    /// # Time
    /// Date and time when the event is recorded.
    pub time: DateTime<Utc>,

    /// # Duration
    /// Duration for which the application window was in the foreground.
    pub duration: TimeDelta,

    /// # Host name
    /// Name of the desktop/phone where the event was sampled.
    pub hostname: String,

    /// # User name
    /// Name of the user logged into the desktop session where the event was sampled (desktop only).
    pub username: Option<String>,

    /// # Idle for
    /// Amount of time since last user interaction at `time` (desktop only).
    pub idle_for: Option<TimeDelta>,

    /// # Program name (unified)
    /// Program name derived from `processName` (desktop) or `applicationLabel` (mobile).
    /// Converted to lowercase. This should be a reasonable
    /// first approximation that allows grouping of the same program across environments.
    pub name: String,

    /// # Program category (unified)
    /// Field for user-defined category. Typically, these will be based on tags, but unlike tags,
    /// each event can only have one category.
    pub category: Option<String>,

    /// # Process path
    /// Absolute path to process binary of the active window (desktop only).
    pub process_path: Option<String>,

    /// # Process name
    /// Last path segment of `processPath`, without file extension like .exe (desktop only).
    pub process_name: Option<String>,

    /// # Application label
    /// Human-readable label of the app to which the activity belongs (mobile only).
    pub application_label: Option<String>,

    /// # Application ID
    /// Dot-separated ID of the app to which the activity belongs (mobile only)
    pub application_id: Option<String>,

    /// # Ignore
    /// Flag that prevents the event being exported. Typically, you will want
    /// to set this via action in `PipelineConfig` by filtering on `IdleForGreaterThanSec`.
    pub ignore: bool,

    /// # Is mobile
    /// Flag that differentiates between event source (desktop/mobile).
    pub is_mobile: bool,

    /// # Tags
    /// Array of string tags that are user-assigned to the event.
    pub tags: Vec<String>,
}

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ActiveEventStringAttribute {
    Hostname,
    Username,
    Name,
    Category,
    ProcessPath,
    ProcessName,
    ApplicationLabel,
    ApplicationId,
}

impl ActiveEventStringAttribute {
    pub fn as_str(&self) -> &str {
        match self {
            ActiveEventStringAttribute::Hostname => { "hostname" }
            ActiveEventStringAttribute::Username => { "username" }
            ActiveEventStringAttribute::Name => { "name" }
            ActiveEventStringAttribute::Category => { "category" }
            ActiveEventStringAttribute::ProcessPath => { "processPath" }
            ActiveEventStringAttribute::ProcessName => { "processName" }
            ActiveEventStringAttribute::ApplicationLabel => { "applicationLabel" }
            ActiveEventStringAttribute::ApplicationId => { "applicationId" }
        }
    }
}
