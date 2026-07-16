use super::*;

pub struct FileLockStore {
    root: Utf8PathBuf,
}

impl FileLockStore {
    pub async fn new(root: impl Into<Utf8PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs_err::tokio::create_dir_all(lock_dir(&root))
            .await
            .map_err(map_store_err)?;
        Ok(Self { root })
    }

    fn path_for(&self, key: &str) -> Utf8PathBuf {
        lock_dir(&self.root).join(format!("{}.json", blake3::hash(key.as_bytes()).to_hex()))
    }
}

#[async_trait]
impl AgentLockStore for FileLockStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let path = self.path_for(key);
        let now = OffsetDateTime::now_utc();
        for _ in 0..3 {
            if let Some(stored) = read_optional_json::<RunLease>(&path).await?
                && stored.expires_at > now
                && stored.owner != owner
            {
                return Ok(None);
            }
            if path.exists() {
                let _ = fs_err::tokio::remove_file(&path).await;
            }
            let lease = RunLease {
                key: key.to_owned(),
                owner: owner.to_owned(),
                acquired_at: now,
                expires_at: now + lease_duration(ttl),
            };
            match create_json(&path, &lease).await {
                Ok(()) => return Ok(Some(lease)),
                Err(err) if err.to_string().contains("File exists") => continue,
                Err(err) => return Err(err),
            }
        }
        Ok(None)
    }

    async fn renew(&self, lease: &RunLease, ttl: Duration) -> Result<bool, StoreError> {
        let path = self.path_for(&lease.key);
        let Some(mut stored) = read_optional_json::<RunLease>(&path).await? else {
            return Ok(false);
        };
        if stored.owner == lease.owner {
            stored.expires_at = OffsetDateTime::now_utc() + lease_duration(ttl);
            write_json(&path, &stored).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn release(&self, lease: RunLease) -> Result<(), StoreError> {
        let path = self.path_for(&lease.key);
        let Some(stored) = read_optional_json::<RunLease>(&path).await? else {
            return Ok(());
        };
        if stored.owner == lease.owner && path.exists() {
            fs_err::tokio::remove_file(path)
                .await
                .map_err(map_store_err)?;
        }
        Ok(())
    }
}
