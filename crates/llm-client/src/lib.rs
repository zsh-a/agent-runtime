//! Runtime-neutral orchestration over a host-owned HTTP transport and clock.
mod client;
mod mock;
mod provider;
mod transport;
mod usage;
pub use bytes::Bytes;
pub use client::*;
pub use llm_protocol::{ApiKey, Endpoint, HttpRequest, Protocol};
pub use mock::*;
pub use provider::*;
pub use transport::*;

#[cfg(test)]
mod tests;
