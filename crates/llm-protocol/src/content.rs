//! Provider-neutral typed content. Unknown block kinds fail at deserialization.
use crate::LlmError;
use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}
impl MessageContent {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Blocks(_) => None,
        }
    }
    pub fn as_array(&self) -> Option<Vec<Value>> {
        match self {
            Self::Text(_) => None,
            Self::Blocks(blocks) => Some(
                blocks
                    .iter()
                    .map(|block| serde_json::to_value(block).expect("typed block serialization"))
                    .collect(),
            ),
        }
    }
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("typed content is JSON serializable")
    }
    pub fn blocks(&self) -> Option<&[ContentBlock]> {
        match self {
            Self::Blocks(b) => Some(b),
            _ => None,
        }
    }
}
impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}
impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        Self::Text(s.into())
    }
}
impl From<Vec<ContentBlock>> for MessageContent {
    fn from(b: Vec<ContentBlock>) -> Self {
        Self::Blocks(b)
    }
}
impl TryFrom<Value> for MessageContent {
    type Error = serde_json::Error;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        serde_json::from_value(v)
    }
}
impl std::fmt::Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_value().fmt(f)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ImageUrl {
        image_url: ImageUrl,
    },
    Document {
        source: ImageSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    InteractionResult {
        interaction_id: String,
        #[serde(flatten)]
        fields: serde_json::Map<String, Value>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageUrl {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 {
        media_type: String,
        #[serde(with = "base64_bytes")]
        #[schemars(with = "String")]
        data: Vec<u8>,
    },
    Url {
        url: String,
    },
}
impl ContentBlock {
    pub fn image(media_type: impl Into<String>, data: Vec<u8>) -> Self {
        Self::Image {
            source: ImageSource::Base64 {
                media_type: media_type.into(),
                data,
            },
        }
    }
    pub fn validate(&self) -> Result<(), LlmError> {
        match self {
            Self::Text { .. }
            | Self::Thinking { .. }
            | Self::RedactedThinking { .. }
            | Self::InteractionResult { .. } => Ok(()),
            Self::Image { source } | Self::Document { source, .. } => match source {
                ImageSource::Base64 { media_type, data }
                    if !data.is_empty()
                        && matches!(
                            media_type.as_str(),
                            "image/jpeg"
                                | "image/png"
                                | "image/gif"
                                | "image/webp"
                                | "application/pdf"
                        ) =>
                {
                    Ok(())
                }
                ImageSource::Url { url }
                    if url.starts_with("https://") || url.starts_with("http://") =>
                {
                    Ok(())
                }
                _ => Err(LlmError::validation("Invalid image/document source")),
            },
            Self::ImageUrl { image_url } if !image_url.url.is_empty() => Ok(()),
            Self::ImageUrl { .. } => Err(LlmError::validation("Image URL is required")),
            Self::ToolUse { id, name, .. } if !id.is_empty() && !name.is_empty() => Ok(()),
            Self::ToolResult { tool_use_id, .. } if !tool_use_id.is_empty() => Ok(()),
            _ => Err(LlmError::validation("Tool id and name must not be empty")),
        }
    }
}
mod base64_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(data: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(data))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}
