use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::{InteractionId, PROTOCOL_VERSION, protocol_version};

/// A durable, host-rendered point where execution must wait for a person.
///
/// The same envelope can represent an `ask_user` choice, a proposal approval,
/// or a typed high-risk confirmation. Business payloads stay opaque; the
/// runtime owns identity, lifecycle, expiry, response validation, and resume
/// routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionEnvelope {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub interaction_id: InteractionId,
    pub kind: InteractionKind,
    #[serde(default)]
    pub mode: InteractionMode,
    #[serde(default)]
    pub status: InteractionStatus,
    pub title: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<InteractionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<InteractionConfirmation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<InteractionSubject>,
    #[serde(default)]
    pub response_schema: Value,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub resume: InteractionResume,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[schemars(with = "Option<String>")]
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<InteractionResponse>,
}

impl InteractionEnvelope {
    pub fn choice(
        title: impl Into<String>,
        prompt: impl Into<String>,
        options: Vec<InteractionOption>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            interaction_id: InteractionId::new_v7(),
            kind: InteractionKind::Choice,
            mode: InteractionMode::OneTap,
            status: InteractionStatus::Pending,
            title: title.into(),
            prompt: prompt.into(),
            options,
            confirmation: None,
            subject: None,
            response_schema: json!({
                "type": "object",
                "required": ["option_id"],
                "properties": {
                    "option_id": {"type": "string"},
                    "custom_text": {"type": "string"}
                }
            }),
            payload: json!({}),
            metadata: json!({}),
            resume: InteractionResume::default(),
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
            response: None,
        }
    }

    pub fn approval(
        title: impl Into<String>,
        prompt: impl Into<String>,
        subject: InteractionSubject,
        mode: InteractionMode,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            interaction_id: InteractionId::new_v7(),
            kind: InteractionKind::Approval,
            mode,
            status: InteractionStatus::Pending,
            title: title.into(),
            prompt: prompt.into(),
            options: Vec::new(),
            confirmation: None,
            subject: Some(subject),
            response_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {"enum": ["approve", "reject", "cancel"]}
                }
            }),
            payload: json!({}),
            metadata: json!({}),
            resume: InteractionResume::default(),
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
            response: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(format!(
                "interaction protocol_version must be '{PROTOCOL_VERSION}'"
            ));
        }
        if self.interaction_id.0.trim().is_empty() {
            return Err("interaction_id must not be empty".to_owned());
        }
        if self.title.trim().is_empty() {
            return Err("interaction title must not be empty".to_owned());
        }
        if !self.response_schema.is_object() {
            return Err("interaction response_schema must be an object".to_owned());
        }
        if !self.payload.is_object() || !self.metadata.is_object() {
            return Err("interaction payload and metadata must be objects".to_owned());
        }
        if self.kind == InteractionKind::Choice {
            if self.options.len() < 2 {
                return Err("choice interaction requires at least two options".to_owned());
            }
            let mut ids = std::collections::BTreeSet::new();
            for option in &self.options {
                if option.id.trim().is_empty() || option.label.trim().is_empty() {
                    return Err("interaction option id and label must not be empty".to_owned());
                }
                if !ids.insert(option.id.as_str()) {
                    return Err(format!("duplicate interaction option '{}'", option.id));
                }
            }
        } else if !self.options.is_empty() {
            return Err("only choice interactions may carry options".to_owned());
        }
        if self.mode == InteractionMode::Typed && self.confirmation.is_none() {
            return Err("typed interaction requires confirmation rules".to_owned());
        }
        match (self.status, self.response.as_ref()) {
            (InteractionStatus::Pending, None)
            | (InteractionStatus::Expired, None)
            | (
                InteractionStatus::Resolved
                | InteractionStatus::Rejected
                | InteractionStatus::Cancelled,
                Some(_),
            ) => {}
            (InteractionStatus::Pending | InteractionStatus::Expired, Some(_)) => {
                return Err("open interaction cannot contain a response".to_owned());
            }
            (
                InteractionStatus::Resolved
                | InteractionStatus::Rejected
                | InteractionStatus::Cancelled,
                None,
            ) => return Err("terminal interaction must contain a response".to_owned()),
        }
        Ok(())
    }

    pub fn is_expired_at(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    pub fn mark_expired_if_needed(&mut self, now: OffsetDateTime) -> bool {
        if self.status == InteractionStatus::Pending && self.is_expired_at(now) {
            self.status = InteractionStatus::Expired;
            return true;
        }
        false
    }

    pub fn resolve(
        &mut self,
        response: InteractionResponse,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        self.validate()?;
        if self.mark_expired_if_needed(now) {
            return Err("interaction has expired".to_owned());
        }
        if self.status != InteractionStatus::Pending {
            return Err("interaction is not pending".to_owned());
        }
        if response.protocol_version != PROTOCOL_VERSION {
            return Err(format!(
                "interaction response protocol_version must be '{PROTOCOL_VERSION}'"
            ));
        }
        if response.interaction_id != self.interaction_id {
            return Err("interaction response id does not match".to_owned());
        }
        self.validate_action(&response)?;
        self.validate_confirmation(&response)?;

        self.status = match response.action {
            InteractionAction::Submit | InteractionAction::Approve => InteractionStatus::Resolved,
            InteractionAction::Reject => InteractionStatus::Rejected,
            InteractionAction::Cancel => InteractionStatus::Cancelled,
        };
        self.response = Some(response);
        self.validate()
    }

    fn validate_action(&self, response: &InteractionResponse) -> Result<(), String> {
        match (self.kind, response.action) {
            (InteractionKind::Input | InteractionKind::Choice, InteractionAction::Submit)
            | (InteractionKind::Approval, InteractionAction::Approve | InteractionAction::Reject)
            | (_, InteractionAction::Cancel) => {}
            _ => return Err("interaction response action is incompatible with kind".to_owned()),
        }
        if self.kind == InteractionKind::Choice && response.action == InteractionAction::Submit {
            let option_id = response
                .value
                .get("option_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let allow_custom = self
                .metadata
                .get("allow_custom")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !self.options.iter().any(|option| option.id == option_id) && !allow_custom {
                return Err("interaction response selects an unknown option".to_owned());
            }
        }
        Ok(())
    }

    fn validate_confirmation(&self, response: &InteractionResponse) -> Result<(), String> {
        let Some(rule) = self.confirmation.as_ref() else {
            return Ok(());
        };
        let supplied = response.confirmation_text.as_deref().unwrap_or_default();
        let matches = if rule.case_sensitive {
            supplied == rule.required_text
        } else {
            supplied.eq_ignore_ascii_case(&rule.required_text)
        };
        if !matches {
            return Err("interaction confirmation text does not match".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Input,
    Choice,
    Approval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    #[default]
    OneTap,
    ConfirmDiff,
    Typed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    #[default]
    Pending,
    Resolved,
    Rejected,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionConfirmation {
    pub required_text: String,
    #[serde(default)]
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionSubject {
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct InteractionResume {
    #[serde(default)]
    pub kind: InteractionResumeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum InteractionResumeKind {
    #[default]
    ChatTurn,
    ProposalApply,
    Host,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionResponse {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub interaction_id: InteractionId,
    pub action: InteractionAction,
    #[serde(default)]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responded_by: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub responded_at: OffsetDateTime,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionAction {
    Submit,
    Approve,
    Reject,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(
        id: &InteractionId,
        action: InteractionAction,
        value: Value,
    ) -> InteractionResponse {
        InteractionResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            interaction_id: id.clone(),
            action,
            value,
            confirmation_text: None,
            responded_by: Some("user-1".to_owned()),
            responded_at: OffsetDateTime::now_utc(),
            metadata: json!({}),
        }
    }

    #[test]
    fn choice_resolves_only_known_options() {
        let mut interaction = InteractionEnvelope::choice(
            "Pick one",
            "Choose a path",
            vec![
                InteractionOption {
                    id: "a".to_owned(),
                    label: "A".to_owned(),
                    description: String::new(),
                    metadata: json!({}),
                },
                InteractionOption {
                    id: "b".to_owned(),
                    label: "B".to_owned(),
                    description: String::new(),
                    metadata: json!({}),
                },
            ],
        );
        let invalid = response(
            &interaction.interaction_id,
            InteractionAction::Submit,
            json!({"option_id": "c"}),
        );
        assert!(
            interaction
                .resolve(invalid, OffsetDateTime::now_utc())
                .is_err()
        );

        let valid = response(
            &interaction.interaction_id,
            InteractionAction::Submit,
            json!({"option_id": "a"}),
        );
        interaction
            .resolve(valid, OffsetDateTime::now_utc())
            .expect("known option resolves");
        assert_eq!(interaction.status, InteractionStatus::Resolved);
    }

    #[test]
    fn typed_approval_requires_exact_confirmation() {
        let subject = InteractionSubject {
            kind: "proposal".to_owned(),
            id: "proposal-1".to_owned(),
            metadata: json!({}),
        };
        let mut interaction = InteractionEnvelope::approval(
            "Approve transfer",
            "Review the diff",
            subject,
            InteractionMode::Typed,
        );
        interaction.confirmation = Some(InteractionConfirmation {
            required_text: "CONFIRM 1000".to_owned(),
            case_sensitive: true,
        });
        let mut invalid = response(
            &interaction.interaction_id,
            InteractionAction::Approve,
            json!({}),
        );
        invalid.confirmation_text = Some("confirm 1000".to_owned());
        assert!(
            interaction
                .resolve(invalid, OffsetDateTime::now_utc())
                .is_err()
        );

        let mut valid = response(
            &interaction.interaction_id,
            InteractionAction::Approve,
            json!({}),
        );
        valid.confirmation_text = Some("CONFIRM 1000".to_owned());
        interaction
            .resolve(valid, OffsetDateTime::now_utc())
            .expect("matching typed confirmation resolves");
        assert_eq!(interaction.status, InteractionStatus::Resolved);
    }

    #[test]
    fn expired_interaction_fails_closed() {
        let mut interaction = InteractionEnvelope::choice(
            "Pick one",
            "",
            vec![
                InteractionOption {
                    id: "a".to_owned(),
                    label: "A".to_owned(),
                    description: String::new(),
                    metadata: json!({}),
                },
                InteractionOption {
                    id: "b".to_owned(),
                    label: "B".to_owned(),
                    description: String::new(),
                    metadata: json!({}),
                },
            ],
        );
        let now = OffsetDateTime::now_utc();
        interaction.expires_at = Some(now - time::Duration::seconds(1));
        let submitted = response(
            &interaction.interaction_id,
            InteractionAction::Submit,
            json!({"option_id": "a"}),
        );
        assert!(interaction.resolve(submitted, now).is_err());
        assert_eq!(interaction.status, InteractionStatus::Expired);
    }
}
