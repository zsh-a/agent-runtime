use super::*;

impl OpenAiSseState {
    pub(super) fn new(
        provider: String,
        model: String,
        response_format: Option<LlmResponseFormat>,
        chunks: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    ) -> Self {
        let mut pending = VecDeque::new();
        pending.push_back(Ok(LlmEvent {
            kind: LlmEventKind::Started,
            content: None,
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            metadata: json!({"provider": provider, "model": model, "stream": true}),
        }));
        Self {
            provider,
            model,
            chunks,
            buffer: String::new(),
            pending,
            content: String::new(),
            finish_reason: None,
            usage: None,
            tools: BTreeMap::new(),
            response_format,
            finished: false,
        }
    }

    pub(super) async fn next_event(&mut self) -> Option<Result<LlmEvent, LlmError>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
            if self.finished {
                return None;
            }
            match self.chunks.next().await {
                Some(Ok(bytes)) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&bytes));
                    self.drain_frames();
                }
                Some(Err(err)) => {
                    self.finished = true;
                    warn!(
                        provider = %self.provider,
                        model = %self.model,
                        error = %err,
                        "OpenAI-compatible stream read failed",
                    );
                    return Some(Err(LlmError::provider(
                        "provider_stream_read_failed",
                        err.to_string(),
                        true,
                        json!({}),
                    )));
                }
                None => {
                    if !self.buffer.trim().is_empty()
                        && let Some(frame) = take_remaining_sse_frame(&mut self.buffer)
                    {
                        self.handle_frame(&frame);
                    }
                    if !self.finished {
                        self.push_finished();
                    }
                }
            }
        }
    }

    pub(super) fn drain_frames(&mut self) {
        while let Some(frame) = take_next_sse_frame(&mut self.buffer) {
            self.handle_frame(&frame);
        }
    }

    pub(super) fn handle_frame(&mut self, frame: &str) {
        let data = sse_data(frame);
        if data.is_empty() {
            return;
        }
        if data.trim() == "[DONE]" {
            self.push_finished();
            return;
        }
        let decoded = match serde_json::from_str::<OpenAiChatCompletionResponse>(&data) {
            Ok(decoded) => decoded,
            Err(err) => {
                warn!(
                    provider = %self.provider,
                    model = %self.model,
                    error = %err,
                    frame_bytes = data.len(),
                    "OpenAI-compatible stream frame decode failed",
                );
                self.pending.push_back(Err(LlmError::provider(
                    "provider_stream_decode_failed",
                    err.to_string(),
                    false,
                    json!({"frame": data}),
                )));
                return;
            }
        };
        if let Some(error) = decoded.error {
            warn!(
                provider = %self.provider,
                model = %self.model,
                error_type = error.r#type.as_deref().unwrap_or("provider_error"),
                "OpenAI-compatible stream returned provider error",
            );
            self.pending.push_back(Err(LlmError::provider(
                error.r#type.unwrap_or_else(|| "provider_error".to_owned()),
                error.message,
                false,
                json!({"code": error.code}),
            )));
            return;
        }
        if let Some(usage) = decoded.usage {
            self.usage = Some(LlmUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            });
        }
        for choice in decoded.choices {
            if let Some(delta) = choice.delta {
                if let Some(content) = delta.content
                    && !content.is_empty()
                {
                    self.content.push_str(&content);
                    self.pending.push_back(Ok(LlmEvent {
                        kind: LlmEventKind::Delta,
                        content: Some(content),
                        response: None,
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({"api": "openai_chat_completions", "stream": true}),
                    }));
                }
                if let Some(reasoning) = delta.reasoning_content.or(delta.reasoning)
                    && !reasoning.is_empty()
                {
                    self.pending.push_back(Ok(LlmEvent {
                        kind: LlmEventKind::ThinkingDelta,
                        content: Some(reasoning),
                        response: None,
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({"api": "openai_chat_completions", "stream": true}),
                    }));
                }
                if let Some(tool_calls) = delta.tool_calls {
                    for tool_call in tool_calls {
                        let index = tool_call.index.unwrap_or(0);
                        let state = self.tools.entry(index).or_default();
                        if let Some(id) = tool_call.id
                            && !id.is_empty()
                        {
                            state.id = id;
                        }
                        if let Some(function) = tool_call.function {
                            if let Some(name) = function.name
                                && !name.is_empty()
                            {
                                state.name = name;
                            }
                            if let Some(arguments) = function.arguments {
                                if !state.started
                                    && (!state.id.is_empty() || !state.name.is_empty())
                                {
                                    state.started = true;
                                    self.pending.push_back(Ok(LlmEvent {
                                        kind: LlmEventKind::ToolCallStart,
                                        content: None,
                                        response: None,
                                        tool_call_id: Some(openai_tool_id(index, state)),
                                        tool_name: Some(state.name.clone()),
                                        partial_input_json: None,
                                        tool_input: None,
                                        metadata: json!({"api": "openai_chat_completions", "stream": true}),
                                    }));
                                }
                                if !arguments.is_empty() {
                                    state.arguments.push_str(&arguments);
                                    self.pending.push_back(Ok(LlmEvent {
                                        kind: LlmEventKind::ToolCallDelta,
                                        content: None,
                                        response: None,
                                        tool_call_id: Some(openai_tool_id(index, state)),
                                        tool_name: Some(state.name.clone()),
                                        partial_input_json: Some(arguments),
                                        tool_input: None,
                                        metadata: json!({"api": "openai_chat_completions", "stream": true}),
                                    }));
                                }
                            } else if !state.started
                                && (!state.id.is_empty() || !state.name.is_empty())
                            {
                                state.started = true;
                                self.pending.push_back(Ok(LlmEvent {
                                    kind: LlmEventKind::ToolCallStart,
                                    content: None,
                                    response: None,
                                    tool_call_id: Some(openai_tool_id(index, state)),
                                    tool_name: Some(state.name.clone()),
                                    partial_input_json: None,
                                    tool_input: None,
                                    metadata: json!({"api": "openai_chat_completions", "stream": true}),
                                }));
                            }
                        }
                    }
                }
            }
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(openai_finish_reason(Some(&reason)));
                if matches!(self.finish_reason, Some(LlmFinishReason::ToolCall)) {
                    self.push_openai_tool_call_ends();
                }
            }
        }
    }

    pub(super) fn push_openai_tool_call_ends(&mut self) {
        for (index, state) in self.tools.iter_mut() {
            if state.ended {
                continue;
            }
            if !state.started {
                state.started = true;
                self.pending.push_back(Ok(LlmEvent {
                    kind: LlmEventKind::ToolCallStart,
                    content: None,
                    response: None,
                    tool_call_id: Some(openai_tool_id(*index, state)),
                    tool_name: Some(state.name.clone()),
                    partial_input_json: None,
                    tool_input: None,
                    metadata: json!({"api": "openai_chat_completions", "stream": true}),
                }));
            }
            state.ended = true;
            self.pending.push_back(Ok(LlmEvent {
                kind: LlmEventKind::ToolCallEnd,
                content: None,
                response: None,
                tool_call_id: Some(openai_tool_id(*index, state)),
                tool_name: Some(state.name.clone()),
                partial_input_json: None,
                tool_input: decode_json_value_or_null(&state.arguments),
                metadata: json!({"api": "openai_chat_completions", "stream": true}),
            }));
        }
    }

    pub(super) fn push_finished(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let object = match structured_output_from_content(&self.response_format, &self.content) {
            Ok(object) => object,
            Err(error) => {
                self.pending.push_back(Err(error));
                return;
            }
        };
        let response = LlmResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            content: self.content.clone(),
            finish_reason: self.finish_reason.clone().unwrap_or(LlmFinishReason::Stop),
            object,
            usage: self.usage.clone(),
            metadata: json!({"api": "openai_chat_completions", "stream": true}),
        };
        info!(
            provider = %self.provider,
            model = %self.model,
            finish_reason = ?response.finish_reason,
            input_tokens = response.usage.as_ref().map(|usage| usage.input_tokens).unwrap_or(0),
            output_tokens = response.usage.as_ref().map(|usage| usage.output_tokens).unwrap_or(0),
            total_tokens = response.usage.as_ref().map(|usage| usage.total_tokens).unwrap_or(0),
            content_chars = response.content.chars().count(),
            "OpenAI-compatible stream finished",
        );
        self.pending.push_back(Ok(LlmEvent {
            kind: LlmEventKind::Finished,
            content: None,
            response: Some(response),
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            metadata: json!({"api": "openai_chat_completions", "stream": true}),
        }));
    }
}
