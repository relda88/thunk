mod groq;
mod llama_cpp;
mod mock;
mod ollama;
mod openai;
mod openrouter;

use crate::core::config::Config;
use crate::core::error::{AppError, Result};
use crate::llm::backend::ModelBackend;

pub use llama_cpp::LlamaCppBackend;

use groq::GroqBackend;
use mock::MockBackend;
use ollama::OllamaBackend;
use openai::OpenAiBackend;
use openrouter::OpenRouterBackend;

type BackendFactory = fn(&Config) -> Result<Box<dyn ModelBackend>>;

fn make_mock(config: &Config) -> Result<Box<dyn ModelBackend>> {
    Ok(Box::new(MockBackend::new(config.app.name.clone())))
}

fn make_llama_cpp(config: &Config) -> Result<Box<dyn ModelBackend>> {
    if config.llama_cpp.model_path.is_none() {
        return Err(AppError::Config(
            "llama_cpp provider requires model_path in config".to_string(),
        ));
    }
    Ok(Box::new(LlamaCppBackend::new(config.llama_cpp.clone())))
}

fn make_openai(config: &Config) -> Result<Box<dyn ModelBackend>> {
    if config.openai.model.is_empty() {
        return Err(AppError::Config(
            "openai provider requires openai.model in config".to_string(),
        ));
    }
    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
        AppError::Config("OPENAI_API_KEY environment variable is not set".to_string())
    })?;
    Ok(Box::new(OpenAiBackend::new(config.openai.clone(), api_key)))
}

fn make_ollama(config: &Config) -> Result<Box<dyn ModelBackend>> {
    Ok(Box::new(OllamaBackend::new(config.ollama.clone())))
}

fn make_openrouter(config: &Config) -> Result<Box<dyn ModelBackend>> {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .ok_or_else(|| AppError::Config("OPENROUTER_API_KEY not set".into()))?;
    Ok(Box::new(OpenRouterBackend::new(
        config.openrouter.clone(),
        api_key,
    )))
}

fn make_groq(config: &Config) -> Result<Box<dyn ModelBackend>> {
    let api_key = std::env::var("GROQ_API_KEY")
        .ok()
        .ok_or_else(|| AppError::Config("GROQ_API_KEY not set".into()))?;
    Ok(Box::new(GroqBackend::new(config.groq.clone(), api_key)))
}

const BACKEND_REGISTRY: &[(&str, BackendFactory)] = &[
    ("mock", make_mock),
    ("llama_cpp", make_llama_cpp),
    ("openai", make_openai),
    ("ollama", make_ollama),
    ("openrouter", make_openrouter),
    ("groq", make_groq),
];

pub fn build_backend(config: &Config) -> Result<Box<dyn ModelBackend>> {
    let name = config.llm.provider.as_str();
    BACKEND_REGISTRY
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, factory)| factory(config))
        .unwrap_or_else(|| {
            let known = BACKEND_REGISTRY
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>()
                .join(", ");
            Err(AppError::Config(format!(
                "Unknown llm.provider `{name}`. Expected one of: {known}."
            )))
        })
}

#[cfg(test)]
mod tests {
    use crate::core::config::{Config, GroqConfig, LlmConfig, OpenAiConfig};
    use crate::core::error::AppError;

    use super::build_backend;

    fn config_with_provider(provider: &str) -> Config {
        Config {
            llm: LlmConfig {
                provider: provider.to_string(),
            },
            ..Default::default()
        }
    }

    fn unwrap_config_err(
        result: crate::core::error::Result<Box<dyn crate::llm::backend::ModelBackend>>,
    ) -> AppError {
        match result {
            Err(e) => e,
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }

    #[test]
    fn llama_cpp_without_model_path_fails_at_startup() {
        let config = config_with_provider("llama_cpp");
        // model_path defaults to None
        let err = unwrap_config_err(build_backend(&config));
        assert!(
            matches!(err, AppError::Config(_)),
            "expected Config error, got: {err}"
        );
        assert!(
            err.to_string().contains("model_path"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn openai_with_empty_model_fails_at_startup() {
        let config = Config {
            llm: LlmConfig {
                provider: "openai".to_string(),
            },
            openai: OpenAiConfig {
                model: String::new(),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = unwrap_config_err(build_backend(&config));
        assert!(
            matches!(err, AppError::Config(_)),
            "expected Config error, got: {err}"
        );
        assert!(
            err.to_string().contains("openai.model"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn openai_without_api_key_fails_at_startup() {
        // Only meaningful when OPENAI_API_KEY is absent; skip if the test environment has it set.
        if std::env::var("OPENAI_API_KEY").is_ok() {
            return;
        }
        let config = Config {
            llm: LlmConfig {
                provider: "openai".to_string(),
            },
            openai: OpenAiConfig {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = unwrap_config_err(build_backend(&config));
        assert!(
            matches!(err, AppError::Config(_)),
            "expected Config error, got: {err}"
        );
        assert!(
            err.to_string().contains("OPENAI_API_KEY"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn groq_config_defaults_to_correct_base_url() {
        let config = GroqConfig::default();
        assert_eq!(config.base_url, "https://api.groq.com/openai/v1");
    }
}
