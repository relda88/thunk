use std::io::BufRead;

use serde_json::{json, Value};

use crate::core::config::OpenAiConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, GenerateRequest, ModelBackend,
};

const DEFAULT_CONTEXT_WINDOW: u32 = 128_000;

pub struct OpenAiBackend {
    config: OpenAiConfig,
    display_name: String,
    api_key: String,
}

impl OpenAiBackend {
    pub fn new(config: OpenAiConfig, api_key: String) -> Self {
        let display_name = format!("openai/{}", config.model);
        Self {
            config,
            display_name,
            api_key,
        }
    }
}

impl ModelBackend for OpenAiBackend {
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
            max_output_tokens: Some(self.config.max_tokens),
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

        let url = format!("{}/chat/completions", self.config.base_url);

        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| AppError::Runtime(format!("OpenAI request failed: {e}")))?;

        on_event(BackendEvent::StatusChanged(BackendStatus::Generating));

        let reader = std::io::BufReader::new(response.into_reader());
        for line in reader.lines() {
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

        on_event(BackendEvent::Finished);
        Ok(())
    }
}
