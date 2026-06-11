mod native;
mod prompt;

use crate::core::config::LlamaCppConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{
    BackendCapabilities, BackendEvent, BackendStatus, BackendTimingStage, ConstrainedMode,
    GenerateRequest, ModelBackend,
};

use native::{load_model, run_generation, LoadedLlama};
use prompt::format_messages;

/// The llama.cpp backed implementation of the ModelBackend.
/// It lazy-loads the model on first use, formats prompts into ChatML, and runs generation
/// while streaming events back to the runtime.
pub struct LlamaCppBackend {
    config: LlamaCppConfig,
    display_name: String,
    loaded: Option<LoadedLlama>,
    last_prompt: Option<String>,
}

impl LlamaCppBackend {
    // Creates a new LlamaCppBackend with the given configuration. The model is not loaded until generate() is called.
    pub fn new(config: LlamaCppConfig) -> Self {
        let model_name = config
            .model_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("unconfigured")
            .to_string();

        Self {
            config,
            display_name: format!("llama.cpp · {model_name}"),
            loaded: None,
            last_prompt: None,
        }
    }

    pub fn last_prompt(&self) -> Option<&str> {
        self.last_prompt.as_deref()
    }

    // Lazily loads the model once and caches it for reuse across requests.
    fn ensure_loaded(&mut self) -> Result<&mut LoadedLlama> {
        if self.loaded.is_none() {
            let model_path = self
                .config
                .model_path
                .clone()
                .expect("model_path validated at startup");
            let loaded = load_model(&self.config, &model_path)?;
            self.loaded = Some(loaded);
        }

        self.loaded
            .as_mut()
            .ok_or_else(|| AppError::Runtime("llama.cpp model failed to initialize.".to_string()))
    }
}

impl ModelBackend for LlamaCppBackend {
    // Returns the display name of the backend, which includes the model name if available.
    fn name(&self) -> &str {
        &self.display_name
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            // context_tokens == 0 means "defer to the model's trained context window at load
            // time" — report None since the true limit is not known until the model is loaded.
            context_window_tokens: if self.config.context_tokens > 0 {
                Some(self.config.context_tokens)
            } else {
                None
            },
            max_output_tokens: Some(self.config.max_tokens),
        }
    }

    // Builds the prompt, ensures the model is loaded, and streams generation events.
    fn generate(
        &mut self,
        request: GenerateRequest,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> Result<()> {
        let config = self.config.clone();
        let grammar = match request.constrained_mode {
            ConstrainedMode::ToolCall => {
                if config.use_grammar {
                    Some(native::TOOL_CALL_GRAMMAR)
                } else {
                    None
                }
            }
            ConstrainedMode::Edit => {
                if config.use_grammar {
                    Some(native::EDIT_GRAMMAR)
                } else {
                    None
                }
            }
            ConstrainedMode::None => None,
        };
        let prompt = format_messages(&request.messages);
        self.last_prompt = Some(prompt.clone());
        on_event(BackendEvent::PromptAssembled(prompt.clone()));
        let is_cold = self.loaded.is_none();
        if is_cold {
            on_event(BackendEvent::StatusChanged(BackendStatus::LoadingModel));
        }
        let t_load_start = is_cold.then(std::time::Instant::now);
        let loaded = self.ensure_loaded()?;
        if let Some(t) = t_load_start {
            on_event(BackendEvent::Timing {
                stage: BackendTimingStage::ModelLoad,
                elapsed_ms: t.elapsed().as_millis() as u64,
            });
        }
        run_generation(loaded, &config, &prompt, grammar, on_event)
    }
}

#[cfg(test)]
mod tests {
    use super::prompt::format_messages;
    use crate::llm::backend::Message;

    #[test]
    fn appends_an_open_assistant_turn() {
        let prompt = format_messages(&[Message::system("system prompt"), Message::user("hello")]);

        assert!(prompt.contains("<|im_start|>system\nsystem prompt<|im_end|>\n"));
        assert!(prompt.contains("<|im_start|>user\nhello<|im_end|>\n"));
        assert!(prompt.ends_with("<|im_start|>assistant\n"));
    }
}
