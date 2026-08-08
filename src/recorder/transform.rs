use crate::core::model::event::ActiveWindowEventV1Data;
use crate::recorder::model::config::{RecorderActiveWindowEventAction, RecorderActiveWindowEventPredicate, RecorderConfig};
use crate::sampler::model::event::{RuntimeActiveWindowEvent, RuntimeEvent};

/// Apply recorder config rules to `RuntimeEvent`
pub fn transform_runtime_event(
    config: &RecorderConfig,
    event: RuntimeEvent
) -> Option<RuntimeEvent> {
    match event {
        RuntimeEvent::ActiveWindowEvent(e) => {
            let output = config.active_window_event_rules.iter().fold(Some(e), |e, rule| {
                e.and_then(|e| {
                    if active_window_event_predicate(&rule.predicate, &e) {
                        rule.actions.iter().fold(Some(e), |e, action| {
                            e.and_then(|e| active_window_event_action(action, e))
                        })
                    } else {
                        Some(e)
                    }
                })
            });
            match output {
                None => None,
                Some(x) => Some(x.into())
            }
        }
    }
}

/// Evaluate `RuntimeActiveWindowEvent` predicate
pub fn active_window_event_predicate(
    p: &RecorderActiveWindowEventPredicate,
    e: &RuntimeActiveWindowEvent
) -> bool {
    match p {
        RecorderActiveWindowEventPredicate::AttributeValue { name: field, value } => {
            match e.extract_string_attribute(field) {
                None => false,
                Some(x) => x == *value
            }
        }
        RecorderActiveWindowEventPredicate::AttributeRegex { name: field, regex } => {
            match e.extract_string_attribute(field) {
                None => false,
                Some(value) => regex.is_match(value.as_str()),
            }
        }
        RecorderActiveWindowEventPredicate::HasTag(tag) => {
            e.data.tags.contains(tag)
        }
        RecorderActiveWindowEventPredicate::And(qs) => {
            qs.iter().all(|q| active_window_event_predicate(q, e))
        }
        RecorderActiveWindowEventPredicate::Or(qs) => {
            qs.iter().any(|q| active_window_event_predicate(q, e))
        }
        RecorderActiveWindowEventPredicate::Not(q) => {
            !active_window_event_predicate(q, e)
        }
    }
}

/// Apply `RuntimeActiveWindowEvent` action
pub fn active_window_event_action(
    action: &RecorderActiveWindowEventAction,
    event: RuntimeActiveWindowEvent
) -> Option<RuntimeActiveWindowEvent> {
    match action {
        RecorderActiveWindowEventAction::AddTag(tag) => Some({
            if event.data.tags.contains(tag) {
                event
            } else {
                let mut new_tags = event.data.tags.clone();
                new_tags.push(tag.clone());
                RuntimeActiveWindowEvent {
                    data: ActiveWindowEventV1Data {
                        tags: new_tags,
                        ..event.data
                    },
                    ..event
                }
            }
        }),
        RecorderActiveWindowEventAction::RedactProcess => Some({
            RuntimeActiveWindowEvent {
                data: ActiveWindowEventV1Data {
                    process_path: None,
                    ..event.data
                },
                ..event
            }
        }),
        RecorderActiveWindowEventAction::Delete => None,
    }
}
