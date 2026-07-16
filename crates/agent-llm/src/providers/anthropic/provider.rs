use super::*;

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        request.validate_protocol()?;
        if request.messages.is_empty() {
            return Err(LlmError::validation(
                "llm request requires at least one message",
            ));
        }
        let started_at = std::time::Instant::now();
        let url = self.messages_url();
        info!(
            provider = %self.provider,
            model = %request.model,
            endpoint = %url,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            temperature = ?request.temperature,
            max_output_tokens = ?request.max_output_tokens,
            anthropic_version = %self.anthropic_version,
            stream = false,
            "starting Anthropic completion request",
        );
        let (system, messages) = anthropic_messages_from_llm(&request.messages)?;
        let system = append_structured_instruction(system, &request.response_format);
        if messages.is_empty() {
            return Err(LlmError::validation(
                "Anthropic request requires at least one user or assistant message",
            ));
        }
        let payload = AnthropicMessagesRequest {
            model: request.model.clone(),
            max_tokens: request.max_output_tokens.unwrap_or(1024),
            messages,
            system,
            temperature: request.temperature,
            tools: request.tools.iter().map(anthropic_tool_from_spec).collect(),
            stream: false,
        };
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.anthropic_version)
            .json(&payload)
            .send()
            .await
            .map_err(|err| {
                LlmError::provider("provider_request_failed", err.to_string(), true, json!({}))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| {
            LlmError::provider(
                "provider_body_read_failed",
                err.to_string(),
                true,
                json!({}),
            )
        })?;
        debug!(
            provider = %self.provider,
            model = %request.model,
            status = %status,
            body_bytes = body.len(),
            duration_ms = started_at.elapsed().as_millis(),
            "Anthropic completion response received",
        );
        if !status.is_success() {
            let details = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
            let message = details
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or(&body)
                .to_owned();
            warn!(
                provider = %self.provider,
                model = %request.model,
                status = %status,
                retryable = status.is_server_error() || status.as_u16() == 429,
                body_preview = %truncate_for_log(&body),
                duration_ms = started_at.elapsed().as_millis(),
                "Anthropic completion failed with non-success status",
            );
            if status.as_u16() == 429 {
                return Err(LlmError::rate_limited(message, details));
            }
            return Err(LlmError::provider(
                format!("provider_http_{}", status.as_u16()),
                message,
                status.is_server_error(),
                details,
            ));
        }
        let decoded = serde_json::from_str::<AnthropicMessagesResponse>(&body).map_err(|err| {
            LlmError::provider(
                "provider_decode_failed",
                err.to_string(),
                false,
                json!({"body": body}),
            )
        })?;
        if let Some(error) = decoded.error {
            warn!(
                provider = %self.provider,
                model = %request.model,
                error_type = error.r#type.as_deref().unwrap_or("provider_error"),
                duration_ms = started_at.elapsed().as_millis(),
                "Anthropic completion returned provider error",
            );
            return Err(LlmError::provider(
                error.r#type.unwrap_or_else(|| "provider_error".to_owned()),
                error.message,
                false,
                json!({}),
            ));
        }
        let raw_content = serde_json::to_value(&decoded.content).map_err(|err| {
            LlmError::provider("provider_decode_failed", err.to_string(), false, json!({}))
        })?;
        let content = anthropic_text_from_blocks(&decoded.content);
        let finish_reason = anthropic_finish_reason(decoded.stop_reason.as_deref());
        let usage = decoded.usage.map(|usage| LlmUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
        });
        let object = structured_output_from_content(&request.response_format, &content)?;
        info!(
            provider = %self.provider,
            model = %request.model,
            finish_reason = ?finish_reason,
            input_tokens = usage.as_ref().map(|usage| usage.input_tokens).unwrap_or(0),
            output_tokens = usage.as_ref().map(|usage| usage.output_tokens).unwrap_or(0),
            total_tokens = usage.as_ref().map(|usage| usage.total_tokens).unwrap_or(0),
            content_chars = content.chars().count(),
            duration_ms = started_at.elapsed().as_millis(),
            "Anthropic completion completed",
        );
        Ok(LlmResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: self.provider.clone(),
            model: request.model,
            content,
            finish_reason,
            object,
            usage,
            metadata: json!({
                "api": "anthropic_messages",
                "anthropic_version": self.anthropic_version,
                "anthropic_content": raw_content,
            }),
        })
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError> {
        request.validate_protocol()?;
        if request.messages.is_empty() {
            return Err(LlmError::validation(
                "llm request requires at least one message",
            ));
        }
        let started_at = std::time::Instant::now();
        let url = self.messages_url();
        info!(
            provider = %self.provider,
            model = %request.model,
            endpoint = %url,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            temperature = ?request.temperature,
            max_output_tokens = ?request.max_output_tokens,
            anthropic_version = %self.anthropic_version,
            stream = true,
            "starting Anthropic stream request",
        );
        let (system, messages) = anthropic_messages_from_llm(&request.messages)?;
        let system = append_structured_instruction(system, &request.response_format);
        if messages.is_empty() {
            return Err(LlmError::validation(
                "Anthropic request requires at least one user or assistant message",
            ));
        }
        let payload = AnthropicMessagesRequest {
            model: request.model.clone(),
            max_tokens: request.max_output_tokens.unwrap_or(1024),
            messages,
            system,
            temperature: request.temperature,
            tools: request.tools.iter().map(anthropic_tool_from_spec).collect(),
            stream: true,
        };
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.anthropic_version)
            .json(&payload)
            .send()
            .await
            .map_err(|err| {
                LlmError::provider("provider_request_failed", err.to_string(), true, json!({}))
            })?;
        let status = response.status();
        debug!(
            provider = %self.provider,
            model = %request.model,
            status = %status,
            duration_ms = started_at.elapsed().as_millis(),
            "Anthropic stream response headers received",
        );
        if !status.is_success() {
            let body = response.text().await.map_err(|err| {
                LlmError::provider(
                    "provider_body_read_failed",
                    err.to_string(),
                    true,
                    json!({}),
                )
            })?;
            let details = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
            let message = details
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or(&body)
                .to_owned();
            warn!(
                provider = %self.provider,
                model = %request.model,
                status = %status,
                retryable = status.is_server_error() || status.as_u16() == 429,
                body_preview = %truncate_for_log(&body),
                duration_ms = started_at.elapsed().as_millis(),
                "Anthropic stream failed with non-success status",
            );
            if status.as_u16() == 429 {
                return Err(LlmError::rate_limited(message, details));
            }
            return Err(LlmError::provider(
                format!("provider_http_{}", status.as_u16()),
                message,
                status.is_server_error(),
                details,
            ));
        }
        let state = AnthropicSseState::new(
            self.provider.clone(),
            request.model,
            self.anthropic_version.clone(),
            request.response_format,
            Box::pin(response.bytes_stream()),
        );
        Ok(Box::pin(stream::unfold(state, |mut state| async move {
            state.next_event().await.map(|event| (event, state))
        })))
    }
}
