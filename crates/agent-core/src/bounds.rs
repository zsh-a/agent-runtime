//! Platform-conditional thread-safety bounds.
//!
//! The runtime is `Send`/`Sync`-first because its native hosts run on a
//! multi-threaded Tokio runtime and share the runtime across tasks. On
//! `wasm32-unknown-unknown` there is a single thread and `reqwest`'s fetch
//! backend holds `!Send` types (`Rc<RefCell<…>>`, non-`Send` byte streams), so
//! the same traits must be expressible without those bounds.
//!
//! Writing the bounds once here keeps the two configurations from drifting:
//! every trait and alias that needs thread safety says `MaybeSend`/`MaybeSync`
//! rather than spelling out `Send`/`Sync`, so a change applies to both targets
//! and the compiler still enforces them natively.
//!
//! This mirrors how `async_trait` is applied in the same crates:
//! `async_trait(?Send)` on wasm, plain `async_trait` elsewhere.

use std::future::Future;
use std::pin::Pin;

use futures::Stream;
/// `Send` on native targets, vacuous on `wasm32-unknown-unknown`.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub trait MaybeSend: Send {}
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` on native targets, vacuous on `wasm32-unknown-unknown`.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub trait MaybeSend {}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` on native targets, vacuous on `wasm32-unknown-unknown`.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub trait MaybeSync: Sync {}
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl<T: Sync + ?Sized> MaybeSync for T {}

/// `Sync` on native targets, vacuous on `wasm32-unknown-unknown`.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub trait MaybeSync {}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl<T: ?Sized> MaybeSync for T {}

/// Mirrors `async_trait`'s expansion target: `+ Send` natively, nothing on wasm.
///
/// This is a *type alias*, not a marker trait: a trait object may only add auto
/// traits, so `dyn Future<…> + MaybeSendFuture` would be rejected. Spelled as
/// `MaybeSendFuture<dyn Future<…>>` both targets expand from one source line.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type MaybeBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// See [`MaybeBoxFuture`].
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type MaybeBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// `Pin<Box<dyn Stream<…>>>`, `Send` natively and unconstrained on wasm.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type MaybeBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// See [`MaybeBoxStream`].
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type MaybeBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;
