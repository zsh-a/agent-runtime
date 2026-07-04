mod hooks;
mod lock;
mod loop_core;
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
pub use loop_core::{RunEffectKind, RunLoop};
pub use policy::ExecutionPolicy;
pub use recovery::{RecoveredRun, RecoveryReport, recover_stale_runs};
pub use registry::InMemoryAgentRegistry;
pub use runner::{AgentRunner, RunControl, RunOutcome, run_idempotency_key};
pub use scheduler::AgentScheduler;
pub use services::BasicAgentServices;
pub use subagent::{SubagentRunContext, run_subagent};
pub use trace::MemoryTraceSink;

pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests;
