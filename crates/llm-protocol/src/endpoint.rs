use crate::LlmError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Anthropic,
    OpenAiCompatible,
    OpenAiResponses,
    Ollama,
}
#[derive(Clone)]
pub struct ApiKey(String);
impl ApiKey {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}
#[derive(Clone)]
pub struct Endpoint {
    pub protocol: Protocol,
    pub provider: String,
    pub base_url: String,
    pub anthropic_version: String,
    api_key: ApiKey,
}
impl Endpoint {
    pub fn new(
        protocol: Protocol,
        provider: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, LlmError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let parsed =
            url::Url::parse(&base_url).map_err(|_| LlmError::validation("Invalid provider URL"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(LlmError::validation(
                "Provider URL must use HTTP(S), a host, and no credentials, query, or fragment",
            ));
        }
        let api_key = ApiKey(api_key.into());
        if protocol == Protocol::Anthropic && api_key.expose().is_empty() {
            return Err(LlmError::validation("Anthropic API key is required"));
        }
        Ok(Self {
            protocol,
            provider: provider.into(),
            base_url,
            anthropic_version: "2023-06-01".into(),
            api_key,
        })
    }
    pub fn api_key(&self) -> &ApiKey {
        &self.api_key
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Max,
}
#[derive(Clone)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("protocol", &self.protocol)
            .field("credential", &self.api_key)
            .finish_non_exhaustive()
    }
}
