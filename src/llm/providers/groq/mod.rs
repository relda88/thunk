use std::io::BufRead;

use serde_json::{json, Value};

use crate::core::config::GroqConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, GenerateRequest, ModelBackend,
};

const DEFAULT_CONTEXT_WINDOW: u32 = 131_072;

pub struct GroqBackend {
    config: GroqConfig,
    display_name: String,
    api_key: String,
}

impl GroqBackend {
    pub fn new(config: GroqConfig, api_key: String) -> Self {
        let display_name = format!("groq/{}", config.model);
        Self {
            config,
            display_name,
            api_key,
        }
    }
}

impl ModelBackend for GroqBackend {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            context_window_tokens: Some(
                self.config
                    .context_window_tokens
                    .unwrap_or(DEFAULT_CONTEXT_WINDOW),
            ),
            max_output_tokens: Some(self.config.max_tokens as usize),
        }
    }

    fn generate(
        &mut self,
        request: GenerateRequest,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> Result<()> {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();

        let body = json!({
            "model": self.config.model,
            "messages": messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
            "stream": true,
            "stream_options": {"include_usage": true},
        });

        on_event(BackendEvent::PromptAssembled(body.to_string()));

        let url = format!("{}/chat/completions", self.config.base_url);

        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| AppError::Runtime(format!("Groq request failed: {e}")))?;

        on_event(BackendEvent::StatusChanged(BackendStatus::Generating));

        let reader = std::io::BufReader::new(response.into_reader());
        drive_groq_sse_stream(reader.lines(), on_event)?;

        on_event(BackendEvent::Finished);
        Ok(())
    }
}

/// Drives Groq's OpenAI-compatible SSE stream: parses `data: ` lines, emits `TextDelta`/`TokenCounts`
/// events, and returns `Ok(())` only if a `finish_reason` was observed on some chunk.
///
/// `finish_reason` (not `[DONE]`) is the gate: `[DONE]` only confirms the SSE transport closed
/// cleanly, but a dropped connection produces a clean-looking EOF with no `[DONE]` and no error,
/// which would otherwise be indistinguishable from a genuinely short response. `finish_reason`
/// arrives on the last content-bearing chunk (before the separate usage-only chunk and `[DONE]`),
/// so it is checked independently of the `[DONE]` break rather than only at loop end.
fn drive_groq_sse_stream(
    lines: impl Iterator<Item = std::io::Result<String>>,
    on_event: &mut dyn FnMut(BackendEvent),
) -> Result<()> {
    let mut finish_reason_seen = false;

    for line in lines {
        let line = line.map_err(|e| AppError::Runtime(format!("SSE read error: {e}")))?;

        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };

        if data == "[DONE]" {
            break;
        }

        let Ok(val) = serde_json::from_str::<Value>(data) else {
            continue;
        };

        if let Some(content) = val["choices"][0]["delta"]["content"].as_str() {
            if !content.is_empty() {
                on_event(BackendEvent::TextDelta(content.to_string()));
            }
        }

        if !val["choices"][0]["finish_reason"].is_null() {
            finish_reason_seen = true;
        }

        // Usage chunk arrives as a final SSE event with empty choices before [DONE].
        // Only present when stream_options.include_usage is accepted by the API.
        if let Some(prompt) = val["usage"]["prompt_tokens"].as_u64() {
            let completion = val["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            on_event(BackendEvent::TokenCounts {
                prompt: prompt as u32,
                completion: completion as u32,
            });
        }
    }

    if !finish_reason_seen {
        return Err(AppError::Runtime(
            "Groq stream ended before a finish_reason completion signal was received".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{Config, GroqConfig};

    #[test]
    fn groq_config_is_readable_from_config_struct() {
        let config = Config::default();
        assert_eq!(config.groq.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(config.groq.max_tokens, 512);
        assert!(!config.groq.model.is_empty());
    }

    #[test]
    fn authorization_header_is_bearer_prefixed() {
        let api_key = "sk-test-key-12345";
        let auth_header = format!("Bearer {api_key}");
        assert_eq!(auth_header, "Bearer sk-test-key-12345");
    }

    #[test]
    fn endpoint_url_appends_chat_completions_to_base_url() {
        let config = GroqConfig {
            base_url: "https://api.groq.com/openai/v1".to_string(),
            ..GroqConfig::default()
        };
        let url = format!("{}/chat/completions", config.base_url);
        assert_eq!(url, "https://api.groq.com/openai/v1/chat/completions");
    }

    #[test]
    fn backend_name_is_groq_slash_model() {
        let config = GroqConfig::default();
        let expected = format!("groq/{}", config.model);
        let backend = GroqBackend::new(config, "key".to_string());
        assert_eq!(backend.name(), expected);
    }

    #[test]
    fn prompt_assembled_emitted_before_status_changed() {
        use crate::llm::backend::{BackendEvent, GenerateRequest, Message, ModelBackend};
        let config = GroqConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            model: "test".to_string(),
            ..GroqConfig::default()
        };
        let mut backend = GroqBackend::new(config, "test-key".to_string());
        let request = GenerateRequest::new(vec![Message::user("hi")]);
        let mut events: Vec<BackendEvent> = Vec::new();
        let _ = backend.generate(request, &mut |e| events.push(e));
        let prompt_idx = events
            .iter()
            .position(|e| matches!(e, BackendEvent::PromptAssembled(_)));
        let status_idx = events
            .iter()
            .position(|e| matches!(e, BackendEvent::StatusChanged(_)));
        assert!(prompt_idx.is_some(), "PromptAssembled must be emitted");
        if let Some(si) = status_idx {
            assert!(
                prompt_idx.unwrap() < si,
                "PromptAssembled must precede StatusChanged"
            );
        }
    }

    fn lines_of(raw: &[&str]) -> std::vec::IntoIter<std::io::Result<String>> {
        raw.iter()
            .map(|s| Ok(s.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn clean_stream_with_finish_reason_and_done_succeeds() {
        use crate::llm::backend::BackendEvent;
        let lines = lines_of(&[
            r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":1}}"#,
            "data: [DONE]",
        ]);
        let mut events = Vec::new();
        let result = drive_groq_sse_stream(lines, &mut |e| events.push(e));
        assert!(result.is_ok(), "clean finish_reason + [DONE] must succeed");
        assert!(events
            .iter()
            .any(|e| matches!(e, BackendEvent::TextDelta(t) if t == "hi")));
    }

    #[test]
    fn eof_without_finish_reason_is_err() {
        let lines = lines_of(&[r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#]);
        let mut events = Vec::new();
        let result = drive_groq_sse_stream(lines, &mut |e| events.push(e));
        assert!(
            result.is_err(),
            "stream ending without finish_reason must be an error, not a silent success"
        );
    }

    #[test]
    fn malformed_chunk_then_clean_finish_succeeds() {
        let lines = lines_of(&[
            "data: not valid json",
            r#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            "data: [DONE]",
        ]);
        let mut events = Vec::new();
        let result = drive_groq_sse_stream(lines, &mut |e| events.push(e));
        assert!(
            result.is_ok(),
            "a single malformed chunk must still be skipped, not fatal"
        );
    }

    #[test]
    fn malformed_chunk_then_eof_is_err() {
        let lines = lines_of(&["data: not valid json"]);
        let mut events = Vec::new();
        let result = drive_groq_sse_stream(lines, &mut |e| events.push(e));
        assert!(
            result.is_err(),
            "malformed chunk followed by EOF with no finish_reason must be an error"
        );
    }
}
