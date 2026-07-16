use super::*;

pub(super) async fn write_bundle_json(
    out: &Utf8Path,
    name: &str,
    value: &impl Serialize,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    write_json_file(out.join(name), value).await?;
    files.insert(bundle_file_key(name), name.to_owned());
    Ok(())
}

pub(super) async fn write_redacted_bundle_jsonl(
    out: &Utf8Path,
    name: &str,
    values: &[Value],
    files: &mut BTreeMap<String, String>,
    report: &mut RedactionReport,
) -> Result<()> {
    let mut lines = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let mut value = value.clone();
        redact_json_value(
            &mut value,
            &format!("$.{}[{index}]", bundle_file_key(name)),
            report,
        );
        lines.push(serde_json::to_string(&value).into_diagnostic()?);
    }
    fs_err::tokio::write(out.join(name), format!("{}\n", lines.join("\n")))
        .await
        .into_diagnostic()?;
    files.insert(bundle_file_key(name), name.to_owned());
    Ok(())
}

pub(super) async fn write_redacted_bundle_json(
    out: &Utf8Path,
    name: &str,
    value: &impl Serialize,
    files: &mut BTreeMap<String, String>,
    report: &mut RedactionReport,
) -> Result<()> {
    let mut value = serde_json::to_value(value).into_diagnostic()?;
    redact_json_value(&mut value, "$", report);
    write_bundle_json(out, name, &value, files).await
}

pub(super) fn bundle_file_key(name: &str) -> String {
    name.trim_end_matches(".json")
        .trim_end_matches(".jsonl")
        .to_owned()
}

pub(super) fn redact_json_value(value: &mut Value, path: &str, report: &mut RedactionReport) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                let child_path = format!("{path}.{}", json_path_key(key));
                if is_sensitive_key(key) {
                    if !value.is_null() {
                        *value = Value::String(report.replacement.clone());
                        report.redacted_paths.push(child_path);
                    }
                } else {
                    redact_json_value(value, &child_path, report);
                }
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter_mut().enumerate() {
                redact_json_value(value, &format!("{path}[{index}]"), report);
            }
        }
        _ => {}
    }
}

pub(super) fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "password",
        "passwd",
        "secret",
        "token",
        "access_token",
        "refresh_token",
        "api_key",
        "apikey",
        "jwt",
        "credential",
        "private_key",
        "local_path",
    ]
    .iter()
    .any(|marker| key == *marker || key.ends_with(marker) || key.contains(&format!("{marker}_")))
}

pub(super) fn json_path_key(key: &str) -> String {
    if key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        key.to_owned()
    } else {
        format!("{key:?}")
    }
}
