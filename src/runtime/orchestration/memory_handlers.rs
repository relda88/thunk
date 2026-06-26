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
        self.pending_memory_is_delete = false;
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
            delete: false,
        });
    }

    /// Approve the pending memory proposal — either persist (write) or delete, then clear.
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

        if self.pending_memory_is_delete {
            if let Some(ref mgr) = self.memory_manager {
                let _ = mgr.store.delete_fact(fact.id);
            }
            on_event(RuntimeEvent::MemoryProposalCleared);
            on_event(RuntimeEvent::SystemMessage(format!(
                "Forgotten: {}",
                fact.text
            )));
            return;
        }

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

    /// Handle /memory — list all stored facts grouped with id, category, text, scope.
    pub(super) fn handle_memory_list(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let Some(ref mgr) = self.memory_manager else {
            on_event(RuntimeEvent::SystemMessage(
                "Memory not available.".to_string(),
            ));
            return;
        };
        match mgr.store.list_facts(None) {
            Ok(facts) if facts.is_empty() => {
                on_event(RuntimeEvent::SystemMessage(
                    "No stored memories.".to_string(),
                ));
            }
            Ok(facts) => {
                let mut lines = vec!["Stored memories:".to_string()];
                for f in &facts {
                    let scope_str = f.scope.as_deref().unwrap_or("global");
                    lines.push(format!(
                        "  [{}] ({}) {}\n       scope: {}  salience: {:.2}",
                        f.id, f.category, f.text, scope_str, f.salience
                    ));
                }
                on_event(RuntimeEvent::InfoMessage(lines.join("\n")));
            }
            Err(_) => {
                on_event(RuntimeEvent::SystemMessage(
                    "Failed to read memory store.".to_string(),
                ));
            }
        }
    }

    /// Handle /forget <id> — propose deletion of the fact with the given id.
    pub(super) fn handle_memory_forget(&mut self, id: i64, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let Some(ref mgr) = self.memory_manager else {
            on_event(RuntimeEvent::SystemMessage(
                "Memory not available.".to_string(),
            ));
            return;
        };
        match mgr.store.get_fact(id) {
            Ok(Some(fact)) => {
                self.pending_memory_is_delete = true;
                let text = fact.text.clone();
                let category = fact.category.clone();
                let scope = fact.scope.clone();
                self.pending_memory = Some(fact);
                on_event(RuntimeEvent::MemoryProposalRequired {
                    fact: text,
                    category,
                    scope,
                    source: "user".to_string(),
                    delete: true,
                });
            }
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "No fact with id {id}."
                )));
            }
            Err(_) => {
                on_event(RuntimeEvent::SystemMessage(
                    "Failed to read memory store.".to_string(),
                ));
            }
        }
    }
}
