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

/// Spawn a detached task with the target's scheduling rules.
///
/// `tokio::spawn` always demands a `Send` future, which the wasm fetch backend
/// cannot produce; wasm runs on one thread and uses `spawn_local` instead.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn spawn_detached<F>(future: F)
where
    F: std::future::Future<Output = ()> + MaybeSend + 'static,
{
    tokio::spawn(future);
}

/// See [`spawn_detached`]; the wasm variant does not require `Send`.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn spawn_detached<F>(future: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}

/// A detached task handle.
///
/// Native returns tokio's `JoinHandle`; wasm has no handle to await, because
/// `spawn_local` detaches, so the handle is a no-op type.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type DetachedTask = tokio::task::JoinHandle<()>;

/// See [`DetachedTask`].
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct DetachedTask;

/// See [`DetachedTask`]; wasm tasks are detached, so there is nothing to abort.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl DetachedTask {
    pub fn abort(&self) {}
}

/// Spawn detached work, returning a handle suitable for fire-and-forget tasks.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn spawn_task<F>(future: F) -> DetachedTask
where
    F: std::future::Future<Output = ()> + MaybeSend + 'static,
{
    tokio::spawn(future)
}

/// See [`spawn_task`]; the wasm variant drops `Send` and the handle.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn spawn_task<F>(future: F) -> DetachedTask
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
    DetachedTask
}

/// A group of concurrently running tasks with deterministic join order.
///
/// Native wraps `tokio::task::JoinSet`, which spawns onto the runtime's work
/// pool. wasm has no multithreaded pool, so tasks are driven on the local
/// executor and joined through a shared queue.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type TaskGroup<T> = tokio::task::JoinSet<T>;

/// Native constructor.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn task_group<T: MaybeSend + 'static>() -> TaskGroup<T> {
    tokio::task::JoinSet::new()
}

/// Join failure: tokio's own type natively, a formatted error on wasm.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type TaskJoinError = tokio::task::JoinError;

/// Join failure for the wasm task group; mirrors `tokio::task::JoinError`'s
/// role, which is only ever formatted by callers.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Debug, Clone)]
pub struct TaskJoinError(String);

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl std::fmt::Display for TaskJoinError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl std::error::Error for TaskJoinError {}

/// wasm task group: tasks run concurrently on the local executor.
///
/// `tokio::task::JoinSet` cannot be used here — it spawns onto a
/// multi-threaded pool and requires `Send` futures. This keeps the same
/// observable shape (spawn, then `join_next` in completion order) while
/// polling on the single thread.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub struct TaskGroup<T> {
    pending: std::rc::Rc<std::cell::RefCell<Vec<Pin<Box<dyn std::future::Future<Output = T> + 'static>>>>>,
}

/// See the wasm `TaskGroup`.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn task_group<T: 'static>() -> TaskGroup<T> {
    TaskGroup {
        pending: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl<T: 'static> TaskGroup<T> {
    /// Queue a task; it starts being polled on the next `join_next`.
    pub fn spawn<F>(&mut self, future: F)
    where
        F: std::future::Future<Output = T> + 'static,
    {
        self.pending.borrow_mut().push(Box::pin(future));
    }

    /// Await whichever queued task finishes first.
    pub async fn join_next(&mut self) -> Option<Result<T, TaskJoinError>> {
        let queued: Vec<_> = self.pending.borrow_mut().drain(..).collect();
        if queued.is_empty() {
            return None;
        }
        let (result, _index, remaining) = futures::future::select_all(queued).await;
        self.pending.borrow_mut().extend(remaining);
        // A wasm future cannot panic-free-fail on join; tasks return their
        // value or their own error type.
        Some(Ok(result))
    }

    /// Whether any task is still queued.
    pub fn is_empty(&self) -> bool {
        self.pending.borrow().is_empty()
    }
}

/// A monotonic clock reading.
///
/// `std::time::Instant` panics on `wasm32-unknown-unknown` ("time not
/// implemented on this platform"), so durations must come from the host clock.
/// Natively this is a thin wrapper over `Instant`; on wasm it reads
/// `performance.now()` and starts at process load, which is all the runtime
/// uses it for (elapsed-time metrics in traces and logs).
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[derive(Debug, Clone, Copy)]
pub struct Clock(tokio::time::Instant);

/// See [`Clock`].
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Debug, Clone, Copy)]
pub struct Clock(f64);

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Clock {
    /// Current reading.
    pub fn now() -> Self {
        Clock(tokio::time::Instant::now())
    }

    /// Milliseconds since this reading.
    pub fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl Clock {
    /// Current reading, from the host performance clock.
    pub fn now() -> Self {
        Clock(web_time_now_ms())
    }

    /// Milliseconds since this reading.
    pub fn elapsed_ms(&self) -> u64 {
        (web_time_now_ms() - self.0).max(0.0) as u64
    }
}

/// Milliseconds from the page's monotonic clock.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn web_time_now_ms() -> f64 {
    // `performance.now()` is always present on the browsers we target.
    global_performance().map(|performance| performance.now()).unwrap_or(0.0)
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn global_performance() -> Option<web_sys::Performance> {
    web_sys::window().and_then(|window| window.performance())
}
