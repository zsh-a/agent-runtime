mod cancellation;
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
#[allow(deprecated)]
pub use loop_core::RunLoop;
pub use loop_core::{EffectStepLoop, RunEffectKind};
pub use policy::ExecutionPolicy;
pub use recovery::{RecoveredRun, RecoveryReport, recover_stale_runs};
pub use registry::InMemoryAgentRegistry;
pub use runner::{AgentRunner, RunControl, RunDisposition, RunOutcome, run_idempotency_key};
pub use scheduler::AgentScheduler;
pub use services::{BasicAgentServices, guarded_services};
pub use subagent::{SubagentRunContext, run_subagent};
pub use trace::{MemoryTraceSink, TraceEventBuffer};

pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests;
