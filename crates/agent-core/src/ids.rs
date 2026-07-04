use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct RunId(pub String);

impl RunId {
    pub fn new_v7() -> Self {
        Self(format!("run_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new_v7() -> Self {
        Self(format!("session_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ThreadId(pub String);

impl ThreadId {
    pub fn new_v7() -> Self {
        Self(format!("thread_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct StepId(pub String);

impl StepId {
    pub fn new_v7() -> Self {
        Self(format!("step_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ProposalId(pub String);

impl ProposalId {
    pub fn new_v7() -> Self {
        Self(format!("proposal_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct EffectId(pub String);

impl EffectId {
    pub fn new_v7() -> Self {
        Self(format!("effect_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

impl ToolCallId {
    pub fn new_v7() -> Self {
        Self(format!("tool_{}", Uuid::now_v7()))
    }
}
