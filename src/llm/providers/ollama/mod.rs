use std::io::BufRead;

use serde_json::{json, Value};

use crate::app::config::OllamaConfig;
use crate::app::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, GenerateRequest, ModelBackend,
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
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();

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

        let response = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| AppError::Runtime(format!("Ollama request failed: {e}")))?;

        on_event(BackendEvent::StatusChanged(BackendStatus::Generating));

        let reader = std::io::BufReader::new(response.into_reader());
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
                    on_event(BackendEvent::TextDelta(content.to_string()));
                }
            }

            if obj["done"].as_bool() == Some(true) {
                break;
            }
        }

        on_event(BackendEvent::Finished);
        Ok(())
    }
}
