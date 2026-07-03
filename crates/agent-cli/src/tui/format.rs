use serde::Serialize;
use serde_json::Value;

pub(super) fn pretty_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unprintable json>".to_owned())
}

pub(super) fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}
