use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::RuntimeEvent;
use super::Runtime;

impl Runtime {
    pub(super) fn handle_index_embed(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let Some(ref store) = self.symbol_store else {
            on_event(RuntimeEvent::SystemMessage(
                "embed: not available (no db path)".to_string(),
            ));
            return;
        };
        if self.embedding_provider.is_none() {
            on_event(RuntimeEvent::SystemMessage(
                "embed: no embedding model configured (set retrieval.embedding_model in config)"
                    .to_string(),
            ));
            return;
        }

        let project_root = self.project_root.path().to_string_lossy().to_string();

        let symbols = match store.all_symbols_ranked(&project_root) {
            Ok(s) => s,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "embed: failed to load symbols: {e}"
                )));
                return;
            }
        };

        if symbols.is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "embed: no symbols indexed (run /index build first)".to_string(),
            ));
            return;
        }

        let total = symbols.len();
        let symbols: Vec<(i64, String)> = if total > 2000 {
            trace_runtime_decision(
                on_event,
                "embedding_cap_applied",
                &[("total", total.to_string()), ("cap", "2000".into())],
            );
            on_event(RuntimeEvent::SystemMessage(format!(
                "embed: {total} symbols found — capping at 2000 by confidence score (High first)"
            )));
            symbols.into_iter().take(2000).collect()
        } else {
            symbols
        };

        let configured_model = self
            .retrieval_config
            .embedding_model
            .clone()
            .unwrap_or_default();

        match store.get_embedding_model(&project_root) {
            Ok(Some(ref stored)) if stored != &configured_model => {
                trace_runtime_decision(
                    on_event,
                    "embedding_model_mismatch",
                    &[
                        ("stored", stored.clone()),
                        ("configured", configured_model.clone()),
                    ],
                );
                on_event(RuntimeEvent::SystemMessage(format!(
                    "embed: model changed ({stored} → {configured_model}), clearing old embeddings"
                )));
                if let Err(e) = store.clear_embeddings(&project_root) {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "embed: failed to clear embeddings: {e}"
                    )));
                    return;
                }
            }
            _ => {}
        }

        on_event(RuntimeEvent::SystemMessage(format!(
            "embed: generating embeddings for {} symbols...",
            symbols.len()
        )));

        let chunk_size = 32;
        let mut embedded: Vec<(i64, Vec<f32>, String)> = Vec::with_capacity(symbols.len());

        for (chunk_idx, chunk) in symbols.chunks(chunk_size).enumerate() {
            let texts: Vec<String> = chunk.iter().map(|(_, sig)| sig.clone()).collect();
            on_event(RuntimeEvent::SystemMessage(format!(
                "embed: chunk {}/{} ({} symbols processed)",
                chunk_idx + 1,
                (symbols.len() + chunk_size - 1) / chunk_size,
                chunk_idx * chunk_size,
            )));
            let vecs = match self.embedding_provider.as_ref().unwrap().embed(&texts) {
                Ok(v) => v,
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!("embed: failed: {e}")));
                    return;
                }
            };
            for ((id, _), vec) in chunk.iter().zip(vecs) {
                embedded.push((*id, vec, configured_model.clone()));
            }
        }

        match store.upsert_embeddings(&project_root, &embedded) {
            Ok(()) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "embed: {} embeddings stored",
                    embedded.len()
                )));
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "embed: storage failed: {e}"
                )));
            }
        }
    }
}
