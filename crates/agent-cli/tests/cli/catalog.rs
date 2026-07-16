use super::*;

#[test]
fn catalog_summary_reads_flutter_export_shape() {
    let output = agent_cmd()
        .args([
            "catalog",
            "summary",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("summary is JSON");
    assert_eq!(json["protocol_version"], "agent.v1");
    assert_eq!(json["catalog_version"], "agent_catalog.v1");
    assert_eq!(json["active_domains"], serde_json::json!(["chat"]));
    assert_eq!(json["agent_count"], 1);
    assert_eq!(json["tool_count"], 1);
    assert_eq!(json["proposal_kind_count"], 1);
    assert_eq!(json["prompt_block_count"], 1);
}

#[test]
fn catalog_agents_and_tools_are_printable() {
    let agents = agent_cmd()
        .args([
            "catalog",
            "agents",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let agents: Value = serde_json::from_slice(&agents).expect("agents are JSON");
    assert_eq!(agents[0]["id"], "ai_chat");

    let tools = agent_cmd()
        .args([
            "catalog",
            "tools",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tools: Value = serde_json::from_slice(&tools).expect("tools are JSON");
    assert_eq!(tools[0]["name"], "propose_fake");
    assert_eq!(tools[0]["risk"], "medium");
    assert_eq!(tools[0]["metadata"]["requires_confirmation"], "one_tap");
}

#[test]
fn catalog_prompt_manifest_records_prompt_model_and_block_hashes() {
    let output = agent_cmd()
        .args([
            "catalog",
            "prompt-manifest",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let manifest: Value = serde_json::from_slice(&output).expect("prompt manifest is JSON");
    assert_eq!(manifest["protocol_version"], "agent.v1");
    assert_eq!(manifest["id"], "ai_chat_prompt");
    assert_eq!(manifest["version"], "ai_chat.prompt.v1");
    assert_eq!(manifest["agent_id"], "ai_chat");
    assert_eq!(manifest["model_family"], "anthropic");
    assert_eq!(manifest["provider"], "anthropic");
    assert_eq!(manifest["model"], "stepfun-ai/Step-3.7-Flash");
    assert_eq!(manifest["tool_schema_version"], "chat.tools.v1");
    assert_eq!(
        manifest["blocks"][0]["content_hash"],
        "blake3:f4d4a59a0aed2318f1a9443b2a51a518cc8296305e2f8db1e1192aac1cc7cd02"
    );
}
