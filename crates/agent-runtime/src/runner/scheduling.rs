use super::*;

impl AgentRunner {
    pub async fn tick(&self, request: RunRequest) -> Result<Vec<RunOutcome>, AgentError> {
        let now = OffsetDateTime::now_utc();
        let scope = request_scope(&request)?;
        let mut outcomes = Vec::new();
        let specs = self.registry.list_agents().await?;
        info!(
            agent_count = specs.len(),
            scope = ?scope,
            trigger = ?request.trigger,
            "evaluating scheduled agents",
        );
        for spec in specs {
            let last = self
                .run_store
                .last_run(&spec.id, &scope)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?;
            if self.scheduler.should_fire(&spec, now, last.as_ref()) {
                info!(
                    agent_id = %spec.id,
                    last_run_id = last
                        .as_ref()
                        .map(|run| run.run_id.0.as_str())
                        .unwrap_or("none"),
                    "scheduled agent is due",
                );
                outcomes.push(self.run_once(&spec.id, request.clone()).await?);
            } else {
                debug!(
                    agent_id = %spec.id,
                    last_run_id = last
                        .as_ref()
                        .map(|run| run.run_id.0.as_str())
                        .unwrap_or("none"),
                    "scheduled agent is not due",
                );
            }
        }
        info!(run_count = outcomes.len(), "scheduler tick finished");
        Ok(outcomes)
    }
}
