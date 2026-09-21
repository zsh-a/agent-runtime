use super::*;

pub(super) fn spawn_lease_renewer(
    lock_store: Arc<dyn agent_core::AgentLockStore>,
    lease: RunLease,
    ttl: Duration,
    lease_kind: &'static str,
    subject_id: String,
    cancellation: Option<CancellationToken>,
) -> agent_core::DetachedTask {
    let interval_duration = lease_renewal_interval(ttl);
    agent_core::spawn_task(async move {
        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            match lock_store.renew(&lease, ttl).await {
                Ok(true) => {
                    debug!(
                        lease_kind,
                        subject_id = %subject_id,
                        lock_key = %lease.key,
                        renew_interval_ms = interval_duration.as_millis(),
                        lease_ttl_ms = ttl.as_millis(),
                        "lease renewed",
                    );
                }
                Ok(false) => {
                    warn!(
                        lease_kind,
                        subject_id = %subject_id,
                        lock_key = %lease.key,
                        "lease ownership was lost",
                    );
                    if let Some(cancellation) = &cancellation {
                        cancellation.cancel();
                    }
                    break;
                }
                Err(error) => {
                    warn!(
                        lease_kind,
                        subject_id = %subject_id,
                        lock_key = %lease.key,
                        error = %error,
                        "failed to renew lease",
                    );
                    if let Some(cancellation) = &cancellation {
                        cancellation.cancel();
                    }
                    break;
                }
            }
        }
    })
}

/// Native awaits the aborted task so the renewer cannot outlive its run.
/// wasm tasks are detached with no handle, and the renewer already exits on
/// the cancellation token, so there is nothing to await.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(super) async fn stop_lease_renewer(handle: agent_core::DetachedTask) {
    handle.abort();
    let _ = handle.await;
}

/// See the native definition above.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(super) async fn stop_lease_renewer(_handle: agent_core::DetachedTask) {}

fn lease_renewal_interval(ttl: Duration) -> Duration {
    let interval_ms = (ttl.as_millis() / 3).max(1).min(u64::MAX as u128) as u64;
    Duration::from_millis(interval_ms)
}
