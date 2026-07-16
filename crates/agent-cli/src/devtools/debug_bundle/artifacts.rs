use super::*;

pub(super) async fn materialize_artifact_refs(
    bundle_out: &Utf8Path,
    artifact_refs: &[Value],
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Result<ArtifactMaterializationManifest> {
    let artifact_dir = bundle_out.join("artifacts");
    let mut records = Vec::new();
    for (index, artifact) in artifact_refs.iter().enumerate() {
        records.push(
            materialize_artifact_ref(&artifact_dir, artifact, index, artifact_resolvers).await?,
        );
    }
    let materialized_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .into_diagnostic()?;
    Ok(ArtifactMaterializationManifest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: RUNTIME_VERSION.to_owned(),
        materialized_at,
        mode: if artifact_resolvers.is_some() {
            "local_files_and_artifact_store_resolvers".to_owned()
        } else {
            "local_files_only".to_owned()
        },
        records,
    })
}

pub(super) async fn materialize_artifact_ref(
    artifact_dir: &Utf8Path,
    artifact: &Value,
    index: usize,
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Result<ArtifactMaterializationRecord> {
    let artifact_id = artifact_id_for_record(artifact, index);
    let Some((source, source_path)) = artifact_source_path(artifact, artifact_resolvers) else {
        return Ok(ArtifactMaterializationRecord {
            artifact_id,
            status: "skipped".to_owned(),
            source: None,
            bundled_path: None,
            size_bytes: None,
            blake3: None,
            reason: Some(
                "unsupported artifact source; expected file:// uri, metadata.local_path, or configured artifact store resolver"
                    .to_owned(),
            ),
        });
    };

    let filename = artifact_materialized_filename(&artifact_id, &source_path, index);
    let bundled_path = format!("artifacts/{filename}");
    let destination = artifact_dir.join(&filename);
    match copy_artifact_file(&source_path, &destination).await {
        Ok((size_bytes, blake3_hash)) => Ok(ArtifactMaterializationRecord {
            artifact_id,
            status: "materialized".to_owned(),
            source: Some(source),
            bundled_path: Some(bundled_path),
            size_bytes: Some(size_bytes),
            blake3: Some(blake3_hash),
            reason: None,
        }),
        Err(error) => Ok(ArtifactMaterializationRecord {
            artifact_id,
            status: "failed".to_owned(),
            source: Some(source),
            bundled_path: None,
            size_bytes: None,
            blake3: None,
            reason: Some(error.to_string()),
        }),
    }
}

pub(super) fn artifact_source_path(
    artifact: &Value,
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Option<(String, Utf8PathBuf)> {
    artifact
        .get("metadata")
        .and_then(|metadata| metadata.get("local_path"))
        .and_then(Value::as_str)
        .map(|path| ("metadata.local_path".to_owned(), Utf8PathBuf::from(path)))
        .or_else(|| {
            artifact
                .get("uri")
                .and_then(Value::as_str)
                .and_then(file_uri_path)
                .map(|path| ("file_uri".to_owned(), path))
        })
        .or_else(|| artifact_store_resolver_path(artifact, artifact_resolvers))
}

pub(super) fn artifact_store_resolver_path(
    artifact: &Value,
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Option<(String, Utf8PathBuf)> {
    let artifact_resolvers = artifact_resolvers?;
    let store = artifact.get("store")?;
    let provider = store.get("provider").and_then(Value::as_str)?;
    let provider = provider.trim();
    if provider.is_empty() {
        return None;
    }
    let resolver = artifact_resolvers
        .resolvers
        .iter()
        .find(|resolver| resolver.provider == provider)?;
    let key = store.get("key").and_then(Value::as_str)?;
    let bucket = store.get("bucket").and_then(Value::as_str);
    let path = artifact_store_local_path(&resolver.root, bucket, key)?;
    Some((format!("artifact_store:{provider}"), path))
}

pub(super) fn artifact_store_local_path(
    root: &Utf8Path,
    bucket: Option<&str>,
    key: &str,
) -> Option<Utf8PathBuf> {
    let mut path = root.to_path_buf();
    if let Some(bucket) = bucket.filter(|bucket| !bucket.trim().is_empty()) {
        push_safe_relative_artifact_path(&mut path, bucket)?;
    }
    push_safe_relative_artifact_path(&mut path, key)?;
    Some(path)
}

pub(super) fn push_safe_relative_artifact_path(path: &mut Utf8PathBuf, value: &str) -> Option<()> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('/') || value.contains('\\') {
        return None;
    }
    for segment in value.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            return None;
        }
        path.push(segment);
    }
    Some(())
}

pub(super) fn file_uri_path(uri: &str) -> Option<Utf8PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let path = path.strip_prefix("localhost").unwrap_or(path);
    if !path.starts_with('/') {
        return None;
    }
    percent_decode_utf8(path).map(Utf8PathBuf::from)
}

pub(super) fn percent_decode_utf8(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            decoded.push((hex_value(hi)? << 4) | hex_value(lo)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

pub(super) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) async fn copy_artifact_file(
    source: &Utf8Path,
    destination: &Utf8Path,
) -> Result<(u64, String)> {
    if let Some(parent) = destination.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let mut input = fs_err::tokio::File::open(source)
        .await
        .map_err(|e| miette!("failed to open artifact source {source}: {e}"))?;
    let mut output = fs_err::tokio::File::create(destination)
        .await
        .map_err(|e| miette!("failed to create materialized artifact {destination}: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|e| miette!("failed to read artifact source {source}: {e}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|e| miette!("failed to write materialized artifact {destination}: {e}"))?;
        hasher.update(&buffer[..read]);
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    output
        .flush()
        .await
        .map_err(|e| miette!("failed to flush materialized artifact {destination}: {e}"))?;
    Ok((total, format!("blake3:{}", hasher.finalize().to_hex())))
}

pub(super) fn artifact_id_for_record(artifact: &Value, index: usize) -> String {
    artifact
        .get("artifact_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("artifact_{}", index + 1))
}

pub(super) fn artifact_materialized_filename(
    artifact_id: &str,
    source_path: &Utf8Path,
    index: usize,
) -> String {
    let mut name = sanitize_artifact_filename(artifact_id);
    if name.is_empty() {
        name = format!("artifact_{}", index + 1);
    }
    if !name.contains('.')
        && let Some(extension) = source_path.extension()
    {
        name.push('.');
        name.push_str(extension);
    }
    format!("{:03}_{name}", index + 1)
}

pub(super) fn sanitize_artifact_filename(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}
