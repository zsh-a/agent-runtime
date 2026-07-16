use super::super::*;

impl AgentRunner {
    pub async fn run_workflow(
        &self,
        request: WorkflowRunRequest,
    ) -> Result<WorkflowRunResult, AgentError> {
        agent_core::validate_protocol_version(&request.protocol_version)
            .map_err(AgentError::validation)?;
        let started_at = OffsetDateTime::now_utc();
        let order = workflow_execution_order(&request.nodes)?;
        let scope = workflow_request_scope(&request)?;
        let workflow_id = request.workflow_id.clone();
        let lock_key = workflow_lock_key(&workflow_id, &scope);
        let lease_owner = format!("workflow:{}", RunId::new_v7().0);
        debug!(
            workflow_id = %workflow_id,
            scope = ?scope,
            lock_key,
            lease_ttl_ms = self.policy.lease_ttl().as_millis(),
            "acquiring workflow lease",
        );
        let Some(lease) = self
            .lock_store
            .acquire(&lock_key, &lease_owner, self.policy.lease_ttl())
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?
        else {
            warn!(
                workflow_id = %workflow_id,
                scope = ?scope,
                lock_key,
                "skipping workflow run because active lease exists",
            );
            return Ok(skipped_workflow_result(
                request,
                started_at,
                "workflow_lease_active",
                json!({
                    "lock_key": lock_key,
                    "scope": scope,
                }),
            ));
        };

        let lease_cancellation = CancellationToken::new();
        let lease_renewer = spawn_lease_renewer(
            self.lock_store.clone(),
            lease.clone(),
            self.policy.lease_ttl(),
            "workflow",
            workflow_id.clone(),
            Some(lease_cancellation.clone()),
        );
        let leased_workflow = tokio::select! {
            result = self.run_workflow_locked(request, started_at, order) => result,
            _ = lease_cancellation.cancelled() => {
                Err(AgentError::cancelled("workflow lease ownership was lost"))
            }
        };
        stop_lease_renewer(lease_renewer).await;
        let release_result = self.lock_store.release(lease).await.map_err(|e| {
            error!(
                workflow_id = %workflow_id,
                error = %e,
                "failed to release workflow lease",
            );
            AgentError::internal(e.to_string())
        });
        if release_result.is_ok() {
            debug!(
                workflow_id = %workflow_id,
                "workflow lease released",
            );
        }
        match (leased_workflow, release_result) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }

    async fn run_workflow_locked(
        &self,
        request: WorkflowRunRequest,
        started_at: OffsetDateTime,
        order: Vec<usize>,
    ) -> Result<WorkflowRunResult, AgentError> {
        let planned_run_ids = planned_workflow_run_ids(&request.nodes);
        let root_run_id = request.root_run_id.clone().or_else(|| {
            order
                .first()
                .and_then(|index| planned_run_ids.get(&request.nodes[*index].node_id))
                .cloned()
        });
        let mut node_results: HashMap<String, WorkflowRunNodeResult> = HashMap::new();
        let mut pending: HashSet<usize> = order.iter().copied().collect();
        let mut running = JoinSet::new();
        let mut active_agent_ids = HashSet::new();

        while !pending.is_empty() || !running.is_empty() {
            let mut progressed = false;
            let mut resolved_indexes = Vec::new();

            for index in &order {
                if !pending.contains(index) {
                    continue;
                }
                let node = &request.nodes[*index];
                if !workflow_dependencies_resolved(node, &node_results) {
                    continue;
                }

                let blocked_dependencies = blocked_workflow_dependencies(node, &node_results);
                if !blocked_dependencies.is_empty() {
                    node_results.insert(
                        node.node_id.clone(),
                        skipped_workflow_node_result(node, blocked_dependencies),
                    );
                    resolved_indexes.push(*index);
                    progressed = true;
                    continue;
                }

                if active_agent_ids.contains(&node.agent_id) {
                    continue;
                }
                active_agent_ids.insert(node.agent_id.clone());
                resolved_indexes.push(*index);
                progressed = true;

                let runner = self.workflow_task_runner();
                let node = node.clone();
                let request = request.clone();
                let root_run_id = root_run_id.clone();
                let planned_run_ids = planned_run_ids.clone();
                let dependency_results = node_results.clone();
                running.spawn(async move {
                    let node_id = node.node_id.clone();
                    let agent_id = node.agent_id.clone();
                    let result = runner
                        .run_workflow_node(
                            &node,
                            &request,
                            root_run_id,
                            &planned_run_ids,
                            &dependency_results,
                        )
                        .await;
                    (node_id, agent_id, result)
                });
            }

            for index in resolved_indexes {
                pending.remove(&index);
            }

            if !progressed {
                let Some(joined) = running.join_next().await else {
                    if pending.is_empty() {
                        break;
                    }
                    return Err(AgentError::internal(
                        "workflow DAG scheduler stalled with pending nodes",
                    ));
                };
                let (node_id, agent_id, result) = joined.map_err(|error| {
                    AgentError::internal(format!("workflow node task failed: {error}"))
                })?;
                active_agent_ids.remove(&agent_id);
                node_results.insert(node_id, result);
            }
        }

        let mut ordered_results = Vec::with_capacity(order.len());
        for index in &order {
            let node = &request.nodes[*index];
            let result = node_results.get(&node.node_id).cloned().ok_or_else(|| {
                AgentError::internal(format!(
                    "workflow DAG scheduler did not produce a result for node '{}'",
                    node.node_id
                ))
            })?;
            ordered_results.push(result);
        }

        let mut status = workflow_status(&ordered_results);
        if workflow_needs_compensation(&ordered_results) {
            self.run_workflow_compensations(&request, root_run_id.clone(), &mut ordered_results)
                .await;
            status = workflow_status(&ordered_results);
        }
        Ok(WorkflowRunResult {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: request.workflow_id,
            status,
            started_at,
            finished_at: OffsetDateTime::now_utc(),
            root_run_id,
            nodes: ordered_results,
            metadata: request.metadata,
        })
    }

    async fn run_workflow_node(
        &self,
        node: &WorkflowRunNode,
        request: &WorkflowRunRequest,
        root_run_id: Option<RunId>,
        planned_run_ids: &HashMap<String, RunId>,
        node_results: &HashMap<String, WorkflowRunNodeResult>,
    ) -> WorkflowRunNodeResult {
        let run_id = planned_run_ids.get(&node.node_id).cloned();
        let dependencies = workflow_run_dependencies(node, node_results);
        let (parent_run_id, parent_agent_id) =
            workflow_parent_from_dependencies(node, node_results);
        let input = match workflow_node_input(node, node_results) {
            Ok(input) => input,
            Err(error) => {
                return WorkflowRunNodeResult {
                    node_id: node.node_id.clone(),
                    agent_id: node.agent_id.clone(),
                    status: AgentRunStatus::Failed,
                    run_id,
                    depends_on: node.depends_on.clone(),
                    output: json!({}),
                    error: Some(*error.record),
                    trace: None,
                    compensation: None,
                    metadata: json!({
                        "reason": "input_mapping_failed",
                    }),
                };
            }
        };
        let workflow = RunWorkflow {
            workflow_id: Some(request.workflow_id.clone()),
            root_run_id,
            parent_run_id,
            parent_agent_id,
            dependencies,
            fanout_id: None,
            fanin_id: None,
            compensation: None,
            metadata: json!({
                "workflow_node_id": node.node_id.clone(),
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
            }),
        };
        let run_request = RunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: run_id.clone(),
            input,
            user: request.user.clone(),
            scope: request.scope.clone(),
            trigger: request.trigger.clone(),
            trigger_envelope: request.trigger_envelope.clone(),
            workflow: Some(workflow),
            metadata: json!({
                "source": "workflow_dag",
                "workflow_id": request.workflow_id.clone(),
                "workflow_node_id": node.node_id.clone(),
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
            }),
        };

        match self.run_once(&node.agent_id, run_request).await {
            Ok(outcome) => WorkflowRunNodeResult {
                node_id: node.node_id.clone(),
                agent_id: node.agent_id.clone(),
                status: outcome.result.status,
                run_id: Some(outcome.result.run_id),
                depends_on: node.depends_on.clone(),
                output: outcome.result.output,
                error: outcome.result.error,
                trace: Some(outcome.trace),
                compensation: None,
                metadata: json!({}),
            },
            Err(error) => WorkflowRunNodeResult {
                node_id: node.node_id.clone(),
                agent_id: node.agent_id.clone(),
                status: AgentRunStatus::Failed,
                run_id,
                depends_on: node.depends_on.clone(),
                output: json!({}),
                error: Some(*error.record),
                trace: None,
                compensation: None,
                metadata: json!({}),
            },
        }
    }

    async fn run_workflow_compensations(
        &self,
        request: &WorkflowRunRequest,
        root_run_id: Option<RunId>,
        ordered_results: &mut [WorkflowRunNodeResult],
    ) {
        for index in (0..ordered_results.len()).rev() {
            let result = &ordered_results[index];
            if result.status != AgentRunStatus::Completed {
                continue;
            }
            let Some(compensated_run_id) = result.run_id.clone() else {
                continue;
            };
            let Some(node) = request
                .nodes
                .iter()
                .find(|node| node.node_id == result.node_id)
            else {
                continue;
            };
            let Some(compensation) = node.compensation.as_ref() else {
                continue;
            };
            ordered_results[index].compensation = Some(
                self.run_workflow_compensation_node(
                    request,
                    node,
                    compensation,
                    root_run_id.clone(),
                    compensated_run_id,
                    result.agent_id.clone(),
                )
                .await,
            );
        }
    }

    async fn run_workflow_compensation_node(
        &self,
        request: &WorkflowRunRequest,
        node: &WorkflowRunNode,
        compensation: &agent_core::WorkflowRunNodeCompensation,
        root_run_id: Option<RunId>,
        compensated_run_id: RunId,
        compensated_agent_id: String,
    ) -> WorkflowRunNodeCompensationResult {
        let run_id = compensation.run_id.clone().unwrap_or_else(RunId::new_v7);
        let workflow = RunWorkflow {
            workflow_id: Some(request.workflow_id.clone()),
            root_run_id,
            parent_run_id: Some(compensated_run_id.clone()),
            parent_agent_id: Some(compensated_agent_id),
            dependencies: Vec::new(),
            fanout_id: None,
            fanin_id: None,
            compensation: Some(RunCompensation {
                compensates_run_id: compensated_run_id.clone(),
                strategy: compensation.strategy.clone(),
                metadata: json!({
                    "workflow_node_id": node.node_id.clone(),
                    "compensation_metadata": compensation.metadata.clone(),
                }),
            }),
            metadata: json!({
                "workflow_node_id": node.node_id.clone(),
                "workflow_compensation": true,
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
            }),
        };
        let run_request = RunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: Some(run_id.clone()),
            input: compensation.input.clone(),
            user: request.user.clone(),
            scope: request.scope.clone(),
            trigger: request.trigger.clone(),
            trigger_envelope: request.trigger_envelope.clone(),
            workflow: Some(workflow),
            metadata: json!({
                "source": "workflow_compensation",
                "workflow_id": request.workflow_id.clone(),
                "workflow_node_id": node.node_id.clone(),
                "compensates_run_id": compensated_run_id.0.clone(),
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
                "compensation_metadata": compensation.metadata.clone(),
            }),
        };

        match self.run_once(&compensation.agent_id, run_request).await {
            Ok(outcome) => WorkflowRunNodeCompensationResult {
                agent_id: compensation.agent_id.clone(),
                status: outcome.result.status,
                run_id: Some(outcome.result.run_id),
                output: outcome.result.output,
                error: outcome.result.error,
                trace: Some(outcome.trace),
                metadata: json!({}),
            },
            Err(error) => WorkflowRunNodeCompensationResult {
                agent_id: compensation.agent_id.clone(),
                status: AgentRunStatus::Failed,
                run_id: Some(run_id),
                output: json!({}),
                error: Some(*error.record),
                trace: None,
                metadata: json!({}),
            },
        }
    }
}
