use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{RuntimeEvent, RuntimeRequest};
use super::Runtime;

pub(super) struct PendingEmbedState {
    /// Full symbol set capped to 2000.
    pub(super) symbols: Vec<(i64, String)>,
    /// Index of the next chunk to process (0-based).
    pub(super) chunk_idx: usize,
    /// Accumulated (symbol_id, embedding_vec, model_name) results.
    pub(super) embedded: Vec<(i64, Vec<f32>, String)>,
    /// Total chunk count for progress display.
    pub(super) total_chunks: usize,
    /// Embedding model name captured at setup time.
    pub(super) configured_model: String,
    /// Project root string for store calls.
    pub(super) project_root: String,
}

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
        let total_chunks = (symbols.len() + chunk_size - 1) / chunk_size;
        let total_symbols = symbols.len();

        self.pending_embed = Some(PendingEmbedState {
            symbols,
            chunk_idx: 0,
            embedded: Vec::with_capacity(total_symbols),
            total_chunks,
            configured_model,
            project_root,
        });

        self.handle_index_embed_chunk(on_event);
    }

    pub(super) fn handle_index_embed_chunk(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let chunk_size = 32;

        // Check if this is a stale dispatch after reset.
        let chunk_start = match self.pending_embed.as_ref() {
            Some(s) => s.chunk_idx * chunk_size,
            None => return,
        };

        let symbols_len = self.pending_embed.as_ref().unwrap().symbols.len();

        if chunk_start >= symbols_len {
            // All chunks done — finalize.
            let state = self.pending_embed.take().unwrap();
            let Some(ref store) = self.symbol_store else {
                on_event(RuntimeEvent::SystemMessage(
                    "embed: store unavailable at finalization".to_string(),
                ));
                return;
            };
            match store.upsert_embeddings(&state.project_root, &state.embedded) {
                Ok(()) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "embed: {} embeddings stored",
                        state.embedded.len()
                    )));
                }
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "embed: storage failed: {e}"
                    )));
                }
            }
            return;
        }

        // Emit progress before the HTTP call.
        {
            let state = self.pending_embed.as_ref().unwrap();
            on_event(RuntimeEvent::SystemMessage(format!(
                "embed: chunk {}/{} ({} symbols processed)",
                state.chunk_idx + 1,
                state.total_chunks,
                chunk_start,
            )));
        }

        // Perform the embed call for this chunk.
        let chunk_end = (chunk_start + chunk_size).min(symbols_len);
        let texts: Vec<String> = {
            let state = self.pending_embed.as_ref().unwrap();
            state.symbols[chunk_start..chunk_end]
                .iter()
                .map(|(_, sig)| sig.clone())
                .collect()
        };

        let vecs = match self.embedding_provider.as_ref() {
            Some(p) => match p.embed(&texts) {
                Ok(v) => v,
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!("embed: failed: {e}")));
                    self.pending_embed = None;
                    return;
                }
            },
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "embed: provider unavailable".to_string(),
                ));
                self.pending_embed = None;
                return;
            }
        };

        // Accumulate results and advance chunk index.
        {
            let state = self.pending_embed.as_mut().unwrap();
            let chunk = &state.symbols[chunk_start..chunk_end];
            let model = state.configured_model.clone();
            for ((id, _), vec) in chunk.iter().zip(vecs) {
                state.embedded.push((*id, vec, model.clone()));
            }
            state.chunk_idx += 1;
        }

        // Recurse to process the next chunk.
        self.handle(RuntimeRequest::IndexEmbedChunk, on_event);
    }
}
