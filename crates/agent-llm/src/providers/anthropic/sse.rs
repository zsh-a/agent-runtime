use super::*;

impl AnthropicSseState {
    pub(super) fn new(
        provider: String,
        model: String,
        anthropic_version: String,
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
            anthropic_version,
            chunks,
            buffer: String::new(),
            pending,
            content: String::new(),
            finish_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            raw_blocks: Vec::new(),
            blocks: BTreeMap::new(),
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
                        "Anthropic stream read failed",
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
        if data.is_empty() || data.trim() == "[DONE]" {
            return;
        }
        let decoded = match serde_json::from_str::<AnthropicStreamEvent>(&data) {
            Ok(decoded) => decoded,
            Err(err) => {
                warn!(
                    provider = %self.provider,
                    model = %self.model,
                    error = %err,
                    frame_bytes = data.len(),
                    "Anthropic stream frame decode failed",
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
        match decoded.event_type.as_str() {
            "message_start" => {
                if let Some(usage) = decoded
                    .message
                    .and_then(|message| message.usage)
                    .or(decoded.usage)
                {
                    self.input_tokens = usage.input_tokens;
                    self.output_tokens = usage.output_tokens;
                }
            }
            "content_block_start" => {
                let index = decoded.index.unwrap_or(0);
                if let Some(block) = decoded.content_block {
                    let block_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    match block_type.as_str() {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                self.push_text_delta(text.to_owned());
                            }
                        }
                        "thinking" => {
                            if let Some(text) = block.get("thinking").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                self.push_thinking_delta(text.to_owned());
                            }
                            if let Some(signature) = block.get("signature").and_then(Value::as_str)
                                && !signature.is_empty()
                            {
                                self.push_thinking_signature(signature.to_owned());
                            }
                        }
                        "tool_use" => {
                            let state = AnthropicBlockState {
                                block_type: block_type.clone(),
                                id: block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                name: block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                input: block.get("input").cloned(),
                                partial_input_json: String::new(),
                            };
                            self.pending.push_back(Ok(LlmEvent {
                                kind: LlmEventKind::ToolCallStart,
                                content: None,
                                response: None,
                                tool_call_id: Some(state.id.clone()),
                                tool_name: Some(state.name.clone()),
                                partial_input_json: None,
                                tool_input: None,
                                metadata: json!({"api": "anthropic_messages", "stream": true}),
                            }));
                            self.blocks.insert(index, state);
                        }
                        _ => {}
                    }
                    self.raw_blocks.push(block);
                }
            }
            "content_block_delta" => {
                if let Some(delta) = decoded.delta {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                self.push_text_delta(text.to_owned());
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(text) = delta.get("thinking").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                self.push_thinking_delta(text.to_owned());
                            }
                        }
                        Some("signature_delta") => {
                            if let Some(signature) = delta.get("signature").and_then(Value::as_str)
                                && !signature.is_empty()
                            {
                                self.push_thinking_signature(signature.to_owned());
                            }
                        }
                        Some("input_json_delta") => {
                            let index = decoded.index.unwrap_or(0);
                            let partial = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            let state = self.blocks.entry(index).or_default();
                            state.partial_input_json.push_str(&partial);
                            self.pending.push_back(Ok(LlmEvent {
                                kind: LlmEventKind::ToolCallDelta,
                                content: None,
                                response: None,
                                tool_call_id: Some(state.id.clone()),
                                tool_name: Some(state.name.clone()),
                                partial_input_json: Some(partial),
                                tool_input: None,
                                metadata: json!({"api": "anthropic_messages", "stream": true}),
                            }));
                        }
                        _ => {}
                    }
                }
            }
            "message_delta" => {
                if let Some(delta) = decoded.delta
                    && let Some(reason) = delta.get("stop_reason").and_then(Value::as_str)
                {
                    self.finish_reason = Some(anthropic_finish_reason(Some(reason)));
                }
                if let Some(usage) = decoded.usage {
                    self.output_tokens = usage.output_tokens;
                }
            }
            "content_block_stop" => {
                let index = decoded.index.unwrap_or(0);
                if let Some(state) = self.blocks.remove(&index)
                    && state.block_type == "tool_use"
                {
                    let input = if state.partial_input_json.trim().is_empty() {
                        Some(state.input.unwrap_or_else(|| json!({})))
                    } else {
                        decode_json_value_or_null(&state.partial_input_json)
                    };
                    self.pending.push_back(Ok(LlmEvent {
                        kind: LlmEventKind::ToolCallEnd,
                        content: None,
                        response: None,
                        tool_call_id: Some(state.id),
                        tool_name: Some(state.name),
                        partial_input_json: None,
                        tool_input: input,
                        metadata: json!({"api": "anthropic_messages", "stream": true}),
                    }));
                }
            }
            "message_stop" => self.push_finished(),
            "error" => {
                let error = decoded.error.unwrap_or(AnthropicErrorBody {
                    r#type: Some("provider_error".to_owned()),
                    message: "provider stream error".to_owned(),
                });
                warn!(
                    provider = %self.provider,
                    model = %self.model,
                    error_type = error.r#type.as_deref().unwrap_or("provider_error"),
                    "Anthropic stream returned provider error",
                );
                self.pending.push_back(Err(LlmError::provider(
                    error.r#type.unwrap_or_else(|| "provider_error".to_owned()),
                    error.message,
                    false,
                    json!({}),
                )));
            }
            "ping" => {}
            _ => {}
        }
    }

    pub(super) fn push_text_delta(&mut self, text: String) {
        self.content.push_str(&text);
        self.pending.push_back(Ok(LlmEvent {
            kind: LlmEventKind::Delta,
            content: Some(text),
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            metadata: json!({"api": "anthropic_messages", "stream": true}),
        }));
    }

    pub(super) fn push_thinking_delta(&mut self, text: String) {
        self.pending.push_back(Ok(LlmEvent {
            kind: LlmEventKind::ThinkingDelta,
            content: Some(text),
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            metadata: json!({"api": "anthropic_messages", "stream": true}),
        }));
    }

    pub(super) fn push_thinking_signature(&mut self, signature: String) {
        self.pending.push_back(Ok(LlmEvent {
            kind: LlmEventKind::ThinkingSignatureDelta,
            content: Some(signature),
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            metadata: json!({"api": "anthropic_messages", "stream": true}),
        }));
    }

    pub(super) fn push_finished(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let usage = (self.input_tokens > 0 || self.output_tokens > 0).then_some(LlmUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: self.input_tokens + self.output_tokens,
        });
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
            usage,
            metadata: json!({
                "api": "anthropic_messages",
                "stream": true,
                "anthropic_version": self.anthropic_version,
                "anthropic_content": self.raw_blocks,
            }),
        };
        info!(
            provider = %self.provider,
            model = %self.model,
            finish_reason = ?response.finish_reason,
            input_tokens = response.usage.as_ref().map(|usage| usage.input_tokens).unwrap_or(0),
            output_tokens = response.usage.as_ref().map(|usage| usage.output_tokens).unwrap_or(0),
            total_tokens = response.usage.as_ref().map(|usage| usage.total_tokens).unwrap_or(0),
            content_chars = response.content.chars().count(),
            "Anthropic stream finished",
        );
        self.pending.push_back(Ok(LlmEvent {
            kind: LlmEventKind::Finished,
            content: None,
            response: Some(response),
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            metadata: json!({"api": "anthropic_messages", "stream": true}),
        }));
    }
}
