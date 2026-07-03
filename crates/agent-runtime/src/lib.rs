mod hooks;
mod lock;
mod policy;
mod recovery;
mod registry;
mod runner;
mod scheduler;
mod services;
mod subagent;
mod trace;

pub use hooks::{FnHook, HookInvocation, HookManager, HookRegistration};
pub use lock::InMemoryLockStore;
pub use policy::ExecutionPolicy;
pub use recovery::{RecoveredRun, RecoveryReport, recover_stale_runs};
pub use registry::InMemoryAgentRegistry;
pub use runner::{AgentRunner, RunControl, RunOutcome, run_idempotency_key};
pub use scheduler::AgentScheduler;
pub use services::BasicAgentServices;
pub use subagent::{
    AGENT_RUN_TOOL_NAME, AgentRunToolContext, agent_run_tool_spec, call_agent_run_tool,
    ensure_agent_run_tool,
};
pub use trace::MemoryTraceSink;

pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests;
