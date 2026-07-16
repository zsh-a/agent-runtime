use super::*;

pub(super) async fn run_eval_scoring_hook(
    hook: &EvalScoringHook,
    case: &EvalCase,
    outcome: &RunOutcome,
    checked: &[String],
) -> Result<EvalScoringHookOutcome> {
    let (command, args) = hook
        .command
        .split_first()
        .ok_or_else(|| miette!("eval {} scoring_hook.command cannot be empty", case.id))?;
    let payload = json!({
        "protocol_version": PROTOCOL_VERSION,
        "eval_id": case.id,
        "agent_id": &outcome.result.agent_id,
        "run_id": &outcome.result.run_id,
        "status": &outcome.result.status,
        "checked": checked,
        "result": &outcome.result,
        "trace": &outcome.trace,
    });
    let started_at = time::OffsetDateTime::now_utc();
    let started = std::time::Instant::now();
    let mut child = TokioCommand::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| miette!("failed to spawn scoring hook {command}: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| miette!("scoring hook stdin missing"))?;
    let mut encoded = serde_json::to_vec(&payload).into_diagnostic()?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await.into_diagnostic()?;
    drop(stdin);

    let output = child.wait_with_output().await.into_diagnostic()?;
    let finished_at = time::OffsetDateTime::now_utc();
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if !output.status.success() {
        return Err(miette!(
            "scoring hook exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let result: EvalScoringResult = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette!("failed to parse scoring hook response: {e}"))?;
    let hook_event = HookEvent {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        hook_event: HookEventName::AfterAgentStep,
        hook_kind: HookKind::Process,
        hook_name: "eval.scoring_hook".to_owned(),
        command: Some(hook.command.clone()),
        run_id: Some(outcome.result.run_id.clone()),
        agent_id: Some(outcome.result.agent_id.clone()),
        status: HookInvocationStatus::Completed,
        started_at,
        finished_at,
        duration_ms,
        input: payload,
        output: Some(serde_json::to_value(&result).into_diagnostic()?),
        error: None,
    };
    Ok(EvalScoringHookOutcome { result, hook_event })
}

pub(crate) async fn run_dev_score_hook() -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let Some(line) = lines.next_line().await.into_diagnostic()? else {
        return Err(miette!("score hook expected one JSON line on stdin"));
    };
    let request: Value = serde_json::from_str(&line).into_diagnostic()?;
    let status = request.get("status").and_then(Value::as_str);
    let passed = status == Some("completed");
    let score = if passed { 1.0 } else { 0.0 };
    let response = json!({
        "passed": passed,
        "score": score,
        "comment": format!("dev score hook saw status {:?}", status),
    });
    println!("{}", serde_json::to_string(&response).into_diagnostic()?);
    Ok(())
}
