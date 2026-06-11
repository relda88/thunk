use std::io::BufRead;

use serde_json::{json, Value};

use crate::core::config::MlxConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, GenerateRequest, ModelBackend,
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

        if request.tool_call_mode && self.config.constrained_output {
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
        for line in reader.lines() {
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

            if let Some(prompt) = val["usage"]["prompt_tokens"].as_u64() {
                let completion = val["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                on_event(BackendEvent::TokenCounts {
                    prompt: prompt as u32,
                    completion: completion as u32,
                });
            }
        }

        on_event(BackendEvent::Finished);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::MlxConfig;
    use crate::llm::backend::{BackendEvent, GenerateRequest, Message, ModelBackend};

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
        r.tool_call_mode = true;
        r
    }

    #[test]
    fn constrained_output_and_tool_call_mode_injects_response_format() {
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
    fn tool_call_mode_false_omits_response_format_even_when_constrained() {
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
            "response_format must be absent when tool_call_mode is false"
        );
    }
}
