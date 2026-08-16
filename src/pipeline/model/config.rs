use chrono::TimeDelta;
use regex::Regex;
use schemars::JsonSchema;
use serde_derive::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};
use crate::core::common::{serialize_regex, deserialize_regex};
use crate::pipeline::model::event::ActiveEventStringAttribute;

/// This configuration defines ETL pipeline to prepare all logged events
/// for further analysis.
#[serde_as]
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfig {
    /// # Rules for ActiveEvent
    /// This defines a sequence of rules that are applied in order
    /// to every logged `ActiveEvent` (desktop `ActiveWindowEvent` + mobile `ActiveActivityEvent`).
    pub active_event_rules: Vec<PipelineActiveEventRule>,

    /// # Maximum duration of one `ActiveEvent` (in seconds)
    /// When set, long events derived from `ActiveActivityEvent` will be split into
    /// multiple shorter ones so that data aggregation is easier.
    /// This is only a concern for `ActiveActivityEvent` (which gets true duration
    /// from the Android OS), not for `ActiveWindowEvent` (which is sampled at regular,
    /// short intervals).
    #[serde_as(as = "DurationSeconds<i64>")]
    #[schemars(with = "i64", range(min = 0))]
    pub active_event_max_duration: TimeDelta,
}

impl PipelineConfig {
    pub fn new() -> PipelineConfig {
        PipelineConfig {
            active_event_rules: vec![],
            active_event_max_duration: TimeDelta::minutes(1),
        }
    }
}

/// A single rule which conditionally transforms an `ActiveEvent`
/// (unified desktop `ActiveWindowEvent` and mobile `ActiveActivityEvent`)
/// during the Transform phase of Moonwatch ETL pipeline. For example, you may choose
/// to tag or redact certain events based on process name, window title,
/// etc.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineActiveEventRule {
    /// # Event predicate
    /// A logical expression that the event must fulfill for the rule to activate.
    pub predicate: PipelineActiveEventPredicate,

    /// # Event actions
    /// A sequence of actions that edit the event.
    pub actions: Vec<PipelineActiveEventAction>,
}

/// A predicate for unified `ActiveEvent` in the ETL pipeline
#[serde_as]
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PipelineActiveEventPredicate {
    /// # Match attribute value
    /// This predicate checks value of a given event attribute
    AttributeValue {
        /// # Attribute name
        /// Name of string attribute to be checked.
        name: ActiveEventStringAttribute,

        /// # Value
        /// Attribute value must match exactly.
        value: String,
    },

    AttributeRegex {
        /// # Attribute name
        /// Name of string attribute to be checked.
        name: ActiveEventStringAttribute,

        /// # Regular expression
        /// Attribute value must match this regular expression.
        #[serde(deserialize_with = "deserialize_regex", serialize_with = "serialize_regex")]
        #[schemars(with = "String", extend("format" = "regex"))]
        regex: Regex,
    },

    /// # Has tag
    /// Check if the event is assigned with given tag (the tag name must match exactly).
    HasTag(String),

    /// # Is mobile
    /// Check if the event comes from `ActiveActivityEvent` (a mobile device).
    IsMobile,

    /// # Idle for greater than given amount of seconds (desktop only)
    /// Matches if `idleFor` attribute exceeds given limit in seconds.
    IdleForGreaterThanSec(i64),

    /// # And
    /// All of these predicates must apply at once.
    And(Vec<Box<PipelineActiveEventPredicate>>),

    /// # Or
    /// At least one of these predicates must apply.
    Or(Vec<Box<PipelineActiveEventPredicate>>),

    /// # Not
    /// Invert a predicate.
    Not(Box<PipelineActiveEventPredicate>),
}

/// An action that transforms `ActiveEvent`
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PipelineActiveEventAction {
    /// # Add tag
    /// Assigns a given tag to the event.
    AddTag(String),

    /// # Set attribute
    /// Sets a new value to given attribute (eg. `name` or `category`).
    SetAttribute{
        /// # Attribute name
        /// Name of string attribute to set.
        name: ActiveEventStringAttribute,

        /// # Value
        /// Value that will be assigned to the attribute.
        value: String,
    },

    /// # Ignore event
    /// Removes the event from further processing by setting the `ignore` attribute to true.
    Ignore,
}
