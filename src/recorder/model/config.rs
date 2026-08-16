use chrono::TimeDelta;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};
use crate::sampler::model::event::RuntimeActiveWindowEventStringAttribute;
use crate::core::common::{serialize_regex, deserialize_regex};

/// This configuration defines rules for processing sampled events
/// before they are written to log. At this time, window title is
/// also available - this data is deliberately never logged, so
/// you should react to it here (assign tags, redact events, etc.).
///
/// Note that this changes how the data is captured - changing this
/// configuration only influences future events, not past data.
/// For flexibility, it is best to do as much processing as possible
/// in `PipelineConfig` instead of here, since changes made there
/// are non-destructive to the captured data.
///
/// It makes sense to share this config file among all your machines.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecorderConfig {
    /// # Rules for ActiveWindowEvent
    /// This defines a sequence of rules that are applied in order
    /// to every captured ActiveWindowEvent (ie. a recorded active
    /// window in the foreground of your desktop).
    pub active_window_event_rules: Vec<RecorderActiveWindowEventRule>,
}

impl RecorderConfig {
    pub fn new() -> RecorderConfig {
        RecorderConfig {
            active_window_event_rules: vec![],
        }
    }
}

/// A single rule which conditionally transforms an ActiveWindowEvent
/// before it will be written to disk. For example, you may choose
/// to tag or redact certain events based on process name, window title,
/// etc.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecorderActiveWindowEventRule {
    /// # Event predicate
    /// A logical expression that the event must fulfill for the rule to activate.
    pub predicate: RecorderActiveWindowEventPredicate,

    /// # Event actions
    /// A sequence of actions that edit the event.
    pub actions: Vec<RecorderActiveWindowEventAction>,
}

/// A predicate for ActiveWindowEvent
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RecorderActiveWindowEventPredicate {
    /// # Match attribute value
    /// This predicate checks value of a given event attribute
    AttributeValue {
        /// # Attribute name
        /// Name of string attribute to be checked.
        name: RuntimeActiveWindowEventStringAttribute,

        /// # Value
        /// Attribute value must match exactly.
        value: String,
    },

    AttributeRegex {
        /// # Attribute name
        /// Name of string attribute to be checked.
        name: RuntimeActiveWindowEventStringAttribute,

        /// # Regular expression
        /// Attribute value must match this regular expression.
        #[serde(deserialize_with = "deserialize_regex", serialize_with = "serialize_regex")]
        #[schemars(with = "String", extend("format" = "regex"))]
        regex: Regex,
    },

    /// # Has tag
    /// Check if the event is assigned with given tag (the tag name must match exactly).
    HasTag(String),

    /// # And
    /// All of these predicates must apply at once.
    And(Vec<Box<RecorderActiveWindowEventPredicate>>),

    /// # Or
    /// At least one of these predicates must apply.
    Or(Vec<Box<RecorderActiveWindowEventPredicate>>),

    /// # Not
    /// Invert a predicate.
    Not(Box<RecorderActiveWindowEventPredicate>),
}

/// An action that transforms ActiveWindowEvent
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RecorderActiveWindowEventAction {
    /// # Add tag
    /// Assigns a given tag to the event.
    AddTag(String),

    /// # Redact process
    /// Replaces the `processPath` attribute with `null`.
    RedactProcess,

    /// # Delete
    /// Removes the event entirely, stopping further processing.
    Delete,
}
