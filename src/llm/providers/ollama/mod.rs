use std::io::BufRead;

use serde_json::{json, Value};

use crate::core::config::OllamaConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, GenerateRequest, ModelBackend, Role,
};

pub struct OllamaBackend {
    config: OllamaConfig,
    display_name: String,
}

impl OllamaBackend {
    pub fn new(config: OllamaConfig) -> Self {
        let display_name = format!("ollama/{}", config.model);
        Self {
            config,
            display_name,
        }
    }
}

impl ModelBackend for OllamaBackend {
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
        let mut leading_system_parts: Vec<&str> = Vec::new();
        let mut first_user_seen = false;
        let mut messages: Vec<Value> = Vec::new();

        for m in &request.messages {
            match m.role {
                Role::System => {
                    if first_user_seen {
                        messages.push(json!({
                            "role": "user",
                            "content": format!("[system]: {}", m.content)
                        }));
                    } else {
                        leading_system_parts.push(&m.content);
                    }
                }
                Role::User => {
                    first_user_seen = true;
                    messages.push(json!({ "role": "user", "content": m.content }));
                }
                Role::Assistant => {
                    first_user_seen = true;
                    messages.push(json!({ "role": "assistant", "content": m.content }));
                }
            }
        }

        if !leading_system_parts.is_empty() {
            let merged = leading_system_parts.join("\n\n");
            messages.insert(0, json!({ "role": "system", "content": merged }));
        }

        let body = json!({
            "model": self.config.model,
            "messages": messages,
            "stream": true,
            "options": {
                "num_predict": self.config.max_tokens,
                "temperature": self.config.temperature,
            }
        });

        let url = format!("{}/api/chat", self.config.base_url);

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(5))
            .timeout_read(std::time::Duration::from_secs(120))
            .build();
        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/x-ndjson")
            .send_string(&body.to_string())
            .map_err(|e| AppError::Runtime(format!("Ollama request failed: {e}")))?;

        on_event(BackendEvent::StatusChanged(BackendStatus::Generating));

        let reader = std::io::BufReader::new(response.into_reader());
        let mut token_count = 0usize;
        for line in reader.lines() {
            let line = line.map_err(|e| AppError::Runtime(format!("Ollama read error: {e}")))?;

            if line.trim().is_empty() {
                continue;
            }

            let Ok(obj) = serde_json::from_str::<Value>(&line) else {
                continue;
            };

            if let Some(content) = obj["message"]["content"].as_str() {
                if !content.is_empty() {
                    token_count += 1;
                    on_event(BackendEvent::TextDelta(content.to_string()));
                }
            }

            if obj["done"].as_bool() == Some(true) {
                break;
            }
        }

        if token_count == 0 {
            let mut fallback_body = body.clone();
            fallback_body["stream"] = json!(false);
            let fallback_response = agent
                .post(&url)
                .set("Content-Type", "application/json")
                .set("Accept", "application/json")
                .send_string(&fallback_body.to_string())
                .map_err(|e| AppError::Runtime(format!("Ollama fallback request failed: {e}")))?;

            let fallback_text = fallback_response
                .into_string()
                .map_err(|e| AppError::Runtime(format!("Ollama fallback read error: {e}")))?;

            if let Ok(obj) = serde_json::from_str::<Value>(&fallback_text) {
                if let Some(content) = obj["message"]["content"].as_str() {
                    if !content.is_empty() {
                        on_event(BackendEvent::TextDelta(content.to_string()));
                    }
                }
            }
        }

        on_event(BackendEvent::Finished);
        Ok(())
    }
}
