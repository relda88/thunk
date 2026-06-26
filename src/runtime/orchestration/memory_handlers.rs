use crate::llm::backend::{BackendEvent, GenerateRequest, Message};
use crate::runtime::memory::MemoryManager;
use crate::runtime::protocol::memory_parser::parse_memory_proposals;
use crate::runtime::types::{Activity, RuntimeEvent};
use crate::storage::memory::{MemoryFact, MemorySource};

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
        if self.pending_memory.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "a memory proposal is already pending — ^Y to confirm, ^N to discard".to_string(),
            ));
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
        self.drain_memory_queue(on_event);
    }

    /// Reject the pending memory proposal — clear without persisting.
    pub(super) fn handle_memory_reject(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.pending_memory = None;
        on_event(RuntimeEvent::MemoryProposalCleared);
        on_event(RuntimeEvent::SystemMessage("Discarded.".to_string()));
        self.drain_memory_queue(on_event);
    }

    /// Pop the next fact from pending_memory_queue and surface it as a proposal, if any remain.
    fn drain_memory_queue(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if let Some(next) = self.pending_memory_queue.pop() {
            self.pending_memory_is_delete = false;
            on_event(RuntimeEvent::MemoryProposalRequired {
                fact: next.text.clone(),
                category: next.category.clone(),
                scope: next.scope.clone(),
                source: "reflection".to_string(),
                delete: false,
            });
            self.pending_memory = Some(next);
        }
    }

    /// Handle /remember <fact> — propose the fact for approval.
    pub(super) fn handle_remember(&mut self, fact: String, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if self.pending_memory.is_some() || !self.pending_memory_queue.is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "reflect in progress — approve or reject current proposal first".to_string(),
            ));
            return;
        }
        if fact.trim().is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "Usage: /remember <fact>".to_string(),
            ));
            return;
        }
        let scope = self.memory_write_scope();
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

    /// Handle /reflect — run a generation pass over recent conversation and propose extracted facts.
    pub(super) fn handle_reflect(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if !self.config.memory.enabled {
            on_event(RuntimeEvent::SystemMessage(
                "reflect: memory is disabled".to_string(),
            ));
            on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
            return;
        }
        if self.pending_memory.is_some() || !self.pending_memory_queue.is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "finish the pending memory proposal first — ^Y to confirm, ^N to discard"
                    .to_string(),
            ));
            return;
        }
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        let raw = match self.generate_reflection_text(on_event) {
            Some(t) => t,
            None => {
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                return;
            }
        };
        let proposals = parse_memory_proposals(&raw);
        if proposals.is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "reflect: nothing to remember".to_string(),
            ));
            on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
            return;
        }
        let scope = self.memory_write_scope();
        let mut facts: Vec<MemoryFact> = proposals
            .into_iter()
            .map(|p| {
                MemoryManager::propose_fact(
                    p.text,
                    p.category,
                    scope.clone(),
                    MemorySource::Reflection,
                )
            })
            .collect();
        // Reverse so pop() later yields them in parse order.
        facts.reverse();
        let first = facts.pop().unwrap();
        self.pending_memory_queue = facts;
        self.pending_memory_is_delete = false;
        on_event(RuntimeEvent::MemoryProposalRequired {
            fact: first.text.clone(),
            category: first.category.clone(),
            scope: first.scope.clone(),
            source: "reflection".to_string(),
            delete: false,
        });
        self.pending_memory = Some(first);
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    /// Run a single generation pass to extract candidate memory facts from recent conversation.
    /// Returns the raw model output or None on failure / insufficient history.
    fn generate_reflection_text(
        &mut self,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<String> {
        let history = self.conversation.human_visible_snapshot();
        if history.len() < 2 {
            on_event(RuntimeEvent::SystemMessage(
                "reflect: not enough conversation history".to_string(),
            ));
            return None;
        }
        let conversation_text: String = history
            .iter()
            .map(|m| format!("{}: {}", m.role.as_str(), m.content))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!(
            "You are extracting facts for long-term memory from a conversation.\n\
             Output ONLY facts in this exact format, one per line:\n\
             [REMEMBER: fact text | category]\n\n\
             Categories: preference, project, identity, workflow, general\n\n\
             Rules:\n\
             - No prose, no preamble, no explanation\n\
             - Only facts worth remembering across sessions\n\
             - Omit anything ephemeral or session-specific\n\
             - At most 5 facts\n\
             - If nothing is worth remembering, output nothing\n\n\
             Conversation:\n\
             {conversation_text}"
        );
        let mut messages = self.conversation.pruned_snapshot();
        messages.push(Message::user(prompt));
        let request = GenerateRequest::new(messages);
        let mut result = String::new();
        if self
            .backend
            .generate(request, &mut |event| {
                if let BackendEvent::TextDelta(chunk) = event {
                    result.push_str(&chunk);
                }
            })
            .is_err()
        {
            on_event(RuntimeEvent::SystemMessage(
                "reflect: backend did not respond".to_string(),
            ));
            return None;
        }
        let text = result.trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}
