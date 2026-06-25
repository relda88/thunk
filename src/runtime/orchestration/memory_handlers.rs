use crate::runtime::memory::MemoryManager;
use crate::runtime::types::RuntimeEvent;
use crate::storage::memory::MemorySource;

use super::Runtime;

impl Runtime {
    /// Propose a memory fact for user approval. Stashes the fact and emits the proposal event.
    /// Never persists — caller must await MemoryApprove.
    pub(super) fn propose_memory(
        &mut self,
        fact: String,
        category: String,
        scope: Option<String>,
        source: MemorySource,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if !self.config.memory.enabled {
            return;
        }
        let memory_fact = MemoryManager::propose_fact(
            fact.clone(),
            category.clone(),
            scope.clone(),
            source.clone(),
        );
        self.pending_memory = Some(memory_fact);
        on_event(RuntimeEvent::MemoryProposalRequired {
            fact,
            category,
            scope,
            source: source.as_str().to_string(),
        });
    }

    /// Approve the pending memory proposal — embed, encode, persist, clear.
    pub(super) fn handle_memory_approve(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let fact = match self.pending_memory.take() {
            Some(f) => f,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "No pending memory proposal.".to_string(),
                ));
                return;
            }
        };

        let embedding: Option<Vec<u8>> = self.embedding_provider.as_ref().and_then(|provider| {
            match provider.embed(&[fact.text.clone()]) {
                Ok(mut vecs) if !vecs.is_empty() && !vecs[0].is_empty() => {
                    Some(crate::storage::vector::encode_embedding(&vecs.remove(0)))
                }
                _ => None,
            }
        });
        let model_name = self.config.retrieval.embedding_model.clone();

        if let Some(ref mgr) = self.memory_manager {
            let _ = mgr.store.upsert_fact(
                &fact.text,
                &fact.category,
                fact.scope.as_deref(),
                fact.salience,
                embedding.as_deref(),
                model_name.as_deref(),
                fact.source.clone(),
            );
        }

        on_event(RuntimeEvent::MemoryProposalCleared);
        on_event(RuntimeEvent::SystemMessage(format!(
            "Remembered: {}",
            fact.text
        )));
    }

    /// Reject the pending memory proposal — clear without persisting.
    pub(super) fn handle_memory_reject(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.pending_memory = None;
        on_event(RuntimeEvent::MemoryProposalCleared);
        on_event(RuntimeEvent::SystemMessage("Discarded.".to_string()));
    }

    /// Handle /remember <fact> — propose the fact for approval.
    pub(super) fn handle_remember(&mut self, fact: String, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if fact.trim().is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "Usage: /remember <fact>".to_string(),
            ));
            return;
        }
        let scope = Some(self.project_root.path().to_string_lossy().into_owned());
        self.propose_memory(
            fact,
            "user".to_string(),
            scope,
            MemorySource::User,
            on_event,
        );
    }
}
