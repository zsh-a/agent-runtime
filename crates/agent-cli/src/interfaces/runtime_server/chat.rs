use super::*;

impl RuntimeServer {
    pub(crate) async fn stream_chat_turn(
        &self,
        mut request: ChatTurnRequest,
    ) -> Result<ChatEventStream> {
        if request.agent_id.is_none() {
            request.agent_id = self.default_agent.clone();
        }
        request.tools = validated_chat_tools(&self.catalog, request.tools)?;
        if let Some(agent_id) = request.agent_id.as_deref() {
            request.messages = self
                .composition
                .chat_messages(agent_id, std::mem::take(&mut request.messages));
        }
        apply_default_context_policy(&mut request.context_policy, &self.context_policy);
        info!(
            turn_id = request.turn_id.as_deref().unwrap_or("none"),
            session_id = request.session_id.as_deref().unwrap_or("none"),
            thread_id = request.thread_id.as_deref().unwrap_or("none"),
            agent_id = request.agent_id.as_deref().unwrap_or("none"),
            provider = %request.provider,
            model = %request.model,
            tool_count = request.tools.len(),
            "server chat turn requested",
        );
        let mut chat = self.chat.clone();
        chat.provider = request.provider.clone();
        chat.model = request.model.clone();
        let provider = provider_from_options(&chat)?;
        let thread_id =
            ensure_thread(self.session_store.as_ref(), request.thread_id.as_deref()).await?;
        let services = self.chat_services(ExecutionContext {
            run_id: request
                .turn_id
                .as_ref()
                .map(|id| RunId(format!("chat_{id}")))
                .unwrap_or_else(RunId::new_v7),
            agent_id: request
                .agent_id
                .clone()
                .unwrap_or_else(|| "chat".to_owned()),
            scope: RunScope::Global,
            user: None,
            metadata: request.metadata.clone(),
        });
        let stream = ChatTurnRunner::new(provider, services).stream(request);
        Ok(self.persist_chat_steps(stream, thread_id))
    }

    pub(crate) async fn stream_chat_resume(
        &self,
        mut request: ChatResumeRequest,
    ) -> Result<ChatEventStream> {
        if request.state.agent_id.is_none() {
            request.state.agent_id = self.default_agent.clone();
        }
        request.state.tools = validated_chat_tools(&self.catalog, request.state.tools)?;
        if let Some(agent_id) = request.state.agent_id.as_deref() {
            request.state.messages = self
                .composition
                .chat_messages(agent_id, std::mem::take(&mut request.state.messages));
        }
        apply_default_context_policy(&mut request.state.context_policy, &self.context_policy);
        info!(
            turn_id = request.state.turn_id.as_deref().unwrap_or("none"),
            session_id = request.state.session_id.as_deref().unwrap_or("none"),
            thread_id = request.state.thread_id.as_deref().unwrap_or("none"),
            agent_id = request.state.agent_id.as_deref().unwrap_or("none"),
            provider = %request.state.provider,
            model = %request.state.model,
            tool_result_count = request.tool_results.len(),
            "server chat resume requested",
        );
        let mut chat = self.chat.clone();
        chat.provider = request.state.provider.clone();
        chat.model = request.state.model.clone();
        let provider = provider_from_options(&chat)?;
        let thread_id = ensure_thread(
            self.session_store.as_ref(),
            request.state.thread_id.as_deref(),
        )
        .await?;
        let services = self.chat_services(ExecutionContext {
            run_id: request
                .state
                .turn_id
                .as_ref()
                .map(|id| RunId(format!("chat_{id}")))
                .unwrap_or_else(RunId::new_v7),
            agent_id: request
                .state
                .agent_id
                .clone()
                .unwrap_or_else(|| "chat".to_owned()),
            scope: RunScope::Global,
            user: None,
            metadata: request.state.metadata.clone(),
        });
        let stream = ChatTurnRunner::new(provider, services).resume(request);
        Ok(self.persist_chat_steps(stream, thread_id))
    }

    fn persist_chat_steps(
        &self,
        stream: ChatEventStream,
        thread_id: Option<ThreadId>,
    ) -> ChatEventStream {
        let Some(thread_id) = thread_id else {
            return stream;
        };
        let store = self.session_store.clone();
        Box::pin(stream.then(move |item| {
            let store = store.clone();
            let thread_id = thread_id.clone();
            async move {
                if let Ok(event) = &item
                    && let Err(err) =
                        record_chat_event_step(store.as_ref(), &thread_id, event).await
                {
                    warn!(
                        thread_id = %thread_id.0,
                        error = %err,
                        "failed to record chat session step",
                    );
                }
                item
            }
        }))
    }

    fn chat_services(&self, context: ExecutionContext) -> Arc<dyn AgentServices> {
        let bound = self.services.bind(context.clone());
        guarded_services(bound, context, self.hooks.clone(), CancellationToken::new())
    }
}
