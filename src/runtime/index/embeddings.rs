use serde_json::json;

use crate::core::error::{AppError, Result};

pub trait EmbeddingProvider: Send {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

pub struct OllamaEmbeddingProvider {
    base_url: String,
    model: String,
}

impl OllamaEmbeddingProvider {
    pub fn new(base_url: String, model: String) -> Self {
        Self { base_url, model }
    }
}

impl EmbeddingProvider for OllamaEmbeddingProvider {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embeddings", self.base_url);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(5))
            .timeout_read(std::time::Duration::from_secs(30))
            .build();

        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            let body = json!({ "model": self.model, "prompt": text });
            let response = agent
                .post(&url)
                .set("Content-Type", "application/json")
                .send_string(&body.to_string())
                .map_err(|e| AppError::Runtime(format!("Ollama embed request failed: {e}")))?;

            let raw = response
                .into_string()
                .map_err(|e| AppError::Runtime(format!("Ollama embed read error: {e}")))?;

            let obj: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| AppError::Runtime(format!("Ollama embed parse error: {e}")))?;

            let embedding = obj["embedding"]
                .as_array()
                .ok_or_else(|| {
                    AppError::Runtime("Ollama embed: missing embedding field".to_string())
                })?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect::<Vec<f32>>();

            results.push(embedding);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConstantProvider(Vec<f32>);
    impl EmbeddingProvider for ConstantProvider {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| self.0.clone()).collect())
        }
    }

    #[test]
    fn embed_returns_one_vector_per_text() {
        let p = ConstantProvider(vec![0.1, 0.2, 0.3]);
        let texts = vec!["hello".to_string(), "world".to_string()];
        let result = p.embed(&texts).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(result[1], vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn embed_empty_slice_returns_empty() {
        let p = ConstantProvider(vec![1.0]);
        let result = p.embed(&[]).unwrap();
        assert!(result.is_empty());
    }
}
