use super::*;

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_lock_replaces_expired_lease() {
    let store = SqliteStore::in_memory().await.expect("sqlite opens");
    let key = "sqlite_expired_lock";
    store
        .acquire(key, "owner_1", Duration::from_secs(1))
        .await
        .expect("first acquire checks")
        .expect("first owner acquires");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let replacement = store
        .acquire(key, "owner_2", Duration::from_secs(60))
        .await
        .expect("expired acquire checks")
        .expect("expired lease is replaced");
    assert_eq!(replacement.owner, "owner_2");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_file_lock_allows_one_concurrent_owner() {
    use std::sync::Arc;

    let path = temp_root().join("locks.sqlite");
    let store = Arc::new(SqliteStore::open(&path).await.expect("sqlite opens"));
    let first = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .acquire("sqlite_concurrent_lock", "owner_1", Duration::from_secs(60))
                .await
                .expect("first acquire checks")
        })
    };
    let second = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .acquire("sqlite_concurrent_lock", "owner_2", Duration::from_secs(60))
                .await
                .expect("second acquire checks")
        })
    };
    let first = first.await.expect("first task joins");
    let second = second.await.expect("second task joins");
    assert_eq!(
        usize::from(first.is_some()) + usize::from(second.is_some()),
        1
    );
}

#[tokio::test]
async fn file_lock_store_coordinates_lease_owners() {
    let root = temp_root();
    let store = FileLockStore::new(root).await.expect("store opens");

    let first = store
        .acquire("agent:echo:scope:global", "run_1", Duration::from_secs(60))
        .await
        .expect("lock acquired")
        .expect("first owner gets lease");
    assert_eq!(first.owner, "run_1");

    let contended = store
        .acquire("agent:echo:scope:global", "run_2", Duration::from_secs(60))
        .await
        .expect("contended lock checks");
    assert!(contended.is_none());

    store.release(first).await.expect("lock released");
    let second = store
        .acquire("agent:echo:scope:global", "run_2", Duration::from_secs(60))
        .await
        .expect("second lock acquired")
        .expect("second owner gets released lease");
    assert_eq!(second.owner, "run_2");
}

#[tokio::test]
async fn file_lock_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileLockStore::new(root).await.expect("store opens");
    assert_lock_store_conformance(&store).await;
}
