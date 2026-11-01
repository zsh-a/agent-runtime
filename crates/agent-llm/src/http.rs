// Native transport owns phased deadlines and retries. Browser fetch keeps its
// existing transport semantics without requiring a Tokio timer or Send stream.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod native;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod wasm;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use native::*;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use wasm::*;
