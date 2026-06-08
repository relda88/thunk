use crate::storage::retrieval::RetrievalLogEntry;

use super::super::super::types::RuntimeEvent;
use super::super::turn_state::{TurnContext, TurnState};
use super::Runtime;

impl Runtime {
    pub(super) fn write_retrieval_log(
        &mut self,
        ctx: &TurnContext,
        state: &TurnState,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let Some(ref store) = self.retrieval_log_store else {
            return;
        };
        let project_root = self.project_root.path().to_string_lossy().into_owned();
        let evidence_outcome = if state.investigation.evidence_ready() {
            "met"
        } else {
            "unmet"
        };
        let entry = RetrievalLogEntry {
            project_root: project_root.clone(),
            strategy: ctx.investigation_mode.as_str().to_string(),
            candidates_found: state.investigation.search_candidate_count(),
            reads_accepted: state.investigation.useful_accepted_candidate_reads,
            evidence_outcome: evidence_outcome.to_string(),
            hops_taken: state
                .deepening
                .as_ref()
                .map(|d| d.reads_this_deep_phase)
                .unwrap_or(0),
            vector_augmented: state.investigation.vector_augmented,
        };
        let _ = store.insert(&entry);

        if self.retrieval_warn_emitted {
            return;
        }
        if let Ok(Some(rate)) = store.recent_hit_rate(&project_root, 5) {
            if rate == 0.0 {
                // Suppress the warn when searches produced no candidates at all (index
                // simply wasn't used) vs. when candidates existed but none were accepted.
                if let Ok(last5) = store.last_n(&project_root, 5) {
                    let any_had_candidates = last5.iter().any(|e| e.candidates_found > 0);
                    if any_had_candidates {
                        self.retrieval_warn_emitted = true;
                        on_event(RuntimeEvent::SystemMessage(
                            "Low retrieval hit rate on recent turns — consider running /index build"
                                .to_string(),
                        ));
                    }
                }
            }
        }
    }
}
