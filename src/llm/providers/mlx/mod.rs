use std::io::BufRead;

use serde_json::{json, Value};

use crate::core::config::MlxConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, ConstrainedMode, GenerateRequest,
    ModelBackend,
};

pub struct MlxBackend {
    config: MlxConfig,
    display_name: String,
}

impl MlxBackend {
    pub fn new(config: MlxConfig) -> Self {
        let display_name = format!("mlx/{}", config.model);
        Self {
            config,
            display_name,
        }
    }
}

impl ModelBackend for MlxBackend {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            context_window_tokens: None,
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

        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
            "stream": true,
            "stream_options": {"include_usage": true},
        });

        if request.constrained_mode == ConstrainedMode::ToolCall && self.config.constrained_output {
            body["response_format"] = json!({"type": "json_object"});
        }

        on_event(BackendEvent::PromptAssembled(body.to_string()));

        let url = format!("{}/v1/chat/completions", self.config.base_url);

        let response = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| AppError::Runtime(format!("MLX request failed: {e}")))?;

        on_event(BackendEvent::StatusChanged(BackendStatus::Generating));

        let reader = std::io::BufReader::new(response.into_reader());
        drive_mlx_sse_stream(reader.lines(), on_event)?;

        on_event(BackendEvent::Finished);
        Ok(())
    }
}

/// Drives an OpenAI-compatible SSE stream from the local MLX server: parses `data: ` lines,
/// emits `TextDelta`/`TokenCounts` events, and returns `Ok(())` only if a `finish_reason` was
/// observed on some chunk.
///
/// `finish_reason` (not `[DONE]`) is the gate: `[DONE]` only confirms the SSE transport closed
/// cleanly, but a dropped connection produces a clean-looking EOF with no `[DONE]` and no error,
/// which would otherwise be indistinguishable from a genuinely short response. `finish_reason`
/// arrives on the last content-bearing chunk (before the separate usage-only chunk and `[DONE]`),
/// so it is checked independently of the `[DONE]` break rather than only at loop end.
///
/// This assumes the local MLX server (e.g. `mlx_lm.server`) emits `finish_reason` in compliance
/// with the OpenAI chat-completions schema it mimics; unlike OpenAI/Groq/OpenRouter this is not an
/// externally-owned, independently verifiable contract from this repo — it depends on whichever
/// local server binary is actually running. Verify against a real running server before relying on
/// this in production; a non-compliant server would make every MLX generation fail this gate.
fn drive_mlx_sse_stream(
    lines: impl Iterator<Item = std::io::Result<String>>,
    on_event: &mut dyn FnMut(BackendEvent),
) -> Result<()> {
    let mut finish_reason_seen = false;

    for line in lines {
        let line = line.map_err(|e| AppError::Runtime(format!("MLX SSE read error: {e}")))?;

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
            "MLX stream ended before a finish_reason completion signal was received".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::MlxConfig;
    use crate::llm::backend::{
        BackendEvent, ConstrainedMode, GenerateRequest, Message, ModelBackend,
    };

    fn backend_with_constrained(constrained_output: bool) -> MlxBackend {
        MlxBackend::new(MlxConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            model: "test".to_string(),
            constrained_output,
            ..MlxConfig::default()
        })
    }

    fn tool_call_request() -> GenerateRequest {
        let mut r = GenerateRequest::new(vec![Message::user("hi")]);
        r.constrained_mode = ConstrainedMode::ToolCall;
        r
    }

    #[test]
    fn constrained_output_and_tool_call_injects_response_format() {
        let mut backend = backend_with_constrained(true);
        let mut assembled: Option<String> = None;
        let _ = backend.generate(tool_call_request(), &mut |e| {
            if let BackendEvent::PromptAssembled(s) = e {
                assembled = Some(s);
            }
        });
        let body: Value =
            serde_json::from_str(&assembled.expect("PromptAssembled must fire")).unwrap();
        assert_eq!(
            body["response_format"]["type"].as_str(),
            Some("json_object"),
            "response_format.type must be json_object"
        );
    }

    #[test]
    fn constrained_output_false_omits_response_format() {
        let mut backend = backend_with_constrained(false);
        let mut assembled: Option<String> = None;
        let _ = backend.generate(tool_call_request(), &mut |e| {
            if let BackendEvent::PromptAssembled(s) = e {
                assembled = Some(s);
            }
        });
        let body: Value =
            serde_json::from_str(&assembled.expect("PromptAssembled must fire")).unwrap();
        assert!(
            body["response_format"].is_null(),
            "response_format must be absent when constrained_output is false"
        );
    }

    #[test]
    fn constrained_mode_none_omits_response_format_even_when_constrained() {
        let mut backend = backend_with_constrained(true);
        let request = GenerateRequest::new(vec![Message::user("hi")]);
        let mut assembled: Option<String> = None;
        let _ = backend.generate(request, &mut |e| {
            if let BackendEvent::PromptAssembled(s) = e {
                assembled = Some(s);
            }
        });
        let body: Value =
            serde_json::from_str(&assembled.expect("PromptAssembled must fire")).unwrap();
        assert!(
            body["response_format"].is_null(),
            "response_format must be absent when constrained_mode is None"
        );
    }

    fn lines_of(raw: &[&str]) -> std::vec::IntoIter<std::io::Result<String>> {
        raw.iter()
            .map(|s| Ok(s.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn clean_stream_with_finish_reason_and_done_succeeds() {
        let lines = lines_of(&[
            r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":1}}"#,
            "data: [DONE]",
        ]);
        let mut events = Vec::new();
        let result = drive_mlx_sse_stream(lines, &mut |e| events.push(e));
        assert!(result.is_ok(), "clean finish_reason + [DONE] must succeed");
        assert!(events
            .iter()
            .any(|e| matches!(e, BackendEvent::TextDelta(t) if t == "hi")));
    }

    #[test]
    fn eof_without_finish_reason_is_err() {
        let lines = lines_of(&[r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#]);
        let mut events = Vec::new();
        let result = drive_mlx_sse_stream(lines, &mut |e| events.push(e));
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
        let result = drive_mlx_sse_stream(lines, &mut |e| events.push(e));
        assert!(
            result.is_ok(),
            "a single malformed chunk must still be skipped, not fatal"
        );
    }

    #[test]
    fn malformed_chunk_then_eof_is_err() {
        let lines = lines_of(&["data: not valid json"]);
        let mut events = Vec::new();
        let result = drive_mlx_sse_stream(lines, &mut |e| events.push(e));
        assert!(
            result.is_err(),
            "malformed chunk followed by EOF with no finish_reason must be an error"
        );
    }
}
