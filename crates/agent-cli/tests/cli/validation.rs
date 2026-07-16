use super::*;

#[test]
fn validate_accepts_valid_schema_fixture() {
    let output = agent_cmd()
        .args([
            "validate",
            "../../schemas/run-request.schema.json",
            "../../fixtures/contracts/run-request.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("validation report is JSON");
    assert_eq!(report["valid"], true);
    assert_eq!(report["errors"], serde_json::json!([]));
}

#[test]
fn validate_rejects_invalid_schema_fixture() {
    let output = agent_cmd()
        .args([
            "validate",
            "../../schemas/run-request.schema.json",
            "../../fixtures/contracts/run-request.invalid.missing-protocol-version.json",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("validation report is JSON");
    assert_eq!(report["valid"], false);
    assert!(
        report["errors"]
            .as_array()
            .expect("errors is an array")
            .iter()
            .any(|error| error
                .as_str()
                .unwrap_or_default()
                .contains("protocol_version"))
    );
}
