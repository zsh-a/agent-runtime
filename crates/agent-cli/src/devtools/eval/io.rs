use super::*;

pub(super) fn absolutize_eval_path(base: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

pub(super) fn absolutize_runtime_path(path: Utf8PathBuf) -> Result<Utf8PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir().into_diagnostic()?;
    Utf8PathBuf::from_path_buf(cwd.join(path.as_std_path()))
        .map_err(|path| miette!("non-UTF-8 path: {}", path.display()))
}

pub(super) async fn read_json_file(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
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

pub(super) async fn write_yaml_file(path: Utf8PathBuf, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let bytes = serde_yaml::to_string(value).into_diagnostic()?;
    fs_err::tokio::write(&path, bytes)
        .await
        .map_err(|e| miette!("failed to write YAML at {path}: {e}"))
}
