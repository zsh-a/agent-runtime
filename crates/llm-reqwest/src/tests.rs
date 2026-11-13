use super::*;
use agent_core::{PROTOCOL_VERSION, ToolSpec};
use axum::{Json, Router, routing::post};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;
mod anthropic;
mod ollama;
mod openai;
