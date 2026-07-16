use super::*;

pub(super) async fn read_json_file(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

pub(super) async fn read_artifact_resolver_manifest(
    path: Utf8PathBuf,
) -> Result<DebugArtifactResolverManifest> {
    let value = read_json_file(path.clone()).await?;
    let mut manifest: DebugArtifactResolverManifest = serde_json::from_value(value)
        .map_err(|e| miette!("failed to parse artifact resolver manifest at {path}: {e}"))?;
    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(miette!(
            "artifact resolver manifest at {path} uses unsupported protocol_version '{}'",
            manifest.protocol_version
        ));
    }

    let base_dir = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|| Utf8PathBuf::from("."));
    let mut providers = BTreeSet::new();
    for resolver in &mut manifest.resolvers {
        resolver.provider = resolver.provider.trim().to_owned();
        if resolver.provider.is_empty() {
            return Err(miette!(
                "artifact resolver manifest at {path} contains an empty provider"
            ));
        }
        if !providers.insert(resolver.provider.clone()) {
            return Err(miette!(
                "artifact resolver manifest at {path} contains duplicate provider '{}'",
                resolver.provider
            ));
        }
        if resolver.root.as_str().trim().is_empty() {
            return Err(miette!(
                "artifact resolver manifest at {path} contains an empty root for provider '{}'",
                resolver.provider
            ));
        }
        if resolver.root.is_relative() {
            resolver.root = base_dir.join(&resolver.root);
        }
    }

    Ok(manifest)
}

pub(super) async fn write_json_file(path: Utf8PathBuf, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let bytes = serde_json::to_vec_pretty(value).into_diagnostic()?;
    fs_err::tokio::write(&path, bytes)
        .await
        .map_err(|e| miette!("failed to write JSON at {path}: {e}"))
}
