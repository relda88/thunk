use std::collections::HashSet;

use crate::tools::ToolInput;

use super::super::investigation::investigation::{InvestigationMode, InvestigationState};
use super::super::investigation::prompt_analysis::{
    DirectReadMode, RetrievalIntent, SimpleEditRequest,
};
use super::super::investigation::tool_surface::ToolSurface;
use super::telemetry::{GenerationRoundCause, GenerationRoundLabel, TurnPerformance};
use super::tool_round::SearchBudget;

#[derive(Clone, Copy)]
pub(crate) enum AnswerPhaseKind {
    PostRead,
    InvestigationEvidenceReady,
}

#[derive(Default)]
pub(crate) struct EngineLocalEscalation {
    pub(crate) closed_search_budget_violations: usize,
    pub(crate) fabricated_tool_result_violations: usize,
    pub(crate) malformed_tool_syntax_violations: usize,
    pub(crate) garbled_edit_repair_violations: usize,
}

pub(crate) enum TurnSignal {
    Continue,
    Finish,
    // deferred: async suspend flow
    #[allow(dead_code)]
    Suspend,
}

/// Tracks progress through the iterative deepening phase (deep investigation mode only).
/// Created on the first deepening hop and reset per turn via TurnState::new().
pub(crate) struct DeepeningState {
    /// Number of hop rounds completed so far. Incremented when all promoted candidates
    /// at the current depth are exhausted and a new depth level begins.
    pub(crate) current_hop: usize,
    /// Total number of reads dispatched during the deepening phase this turn.
    pub(crate) reads_this_deep_phase: usize,
}

pub(crate) struct PendingRuntimeCall {
    pub(crate) input: ToolInput,
    pub(crate) seeded_pre_generation: bool,
}

pub(crate) struct TurnContext {
    pub(crate) retrieval_intent: RetrievalIntent,
    pub(crate) requested_read_path: Option<String>,
    pub(crate) direct_read_mode: Option<DirectReadMode>,
    pub(crate) investigation_required: bool,
    pub(crate) mutation_allowed: bool,
    pub(crate) simple_edit_request: Option<SimpleEditRequest>,
    pub(crate) tool_surface: ToolSurface,
    pub(crate) investigation_mode: InvestigationMode,
    pub(crate) investigation_path_scope: Option<String>,
    pub(crate) shell_request: Option<String>,
}

pub(crate) struct TurnState {
    pub(crate) tool_rounds: usize,
    pub(crate) reads_this_turn: HashSet<String>,
    pub(crate) corrections: usize,
    pub(crate) escalation: EngineLocalEscalation,
    pub(crate) last_call_key: Option<String>,
    pub(crate) pending_runtime_call: Option<PendingRuntimeCall>,
    pub(crate) search_budget: SearchBudget,
    pub(crate) investigation: InvestigationState,
    pub(crate) turn_perf: TurnPerformance,
    pub(crate) next_round_label: GenerationRoundLabel,
    pub(crate) next_round_cause: GenerationRoundCause,
    pub(crate) requested_read_completed: bool,
    pub(crate) read_request_correction_issued: bool,
    pub(crate) disallowed_tool_attempts: usize,
    pub(crate) weak_search_query_attempts: usize,
    pub(crate) answer_phase: Option<AnswerPhaseKind>,
    pub(crate) post_answer_phase_tool_attempts: usize,
    pub(crate) post_answer_phase_correction_echo_retries: usize,
    pub(crate) seeded_tool_executed: bool,
    pub(crate) direct_read_result: Option<String>,
    pub(crate) answer_guard_retry_entered: bool,
    /// Deepening phase state; only populated when investigation_depth == Deep and the
    /// initial candidate reads are exhausted without satisfying evidence gates.
    pub(crate) deepening: Option<DeepeningState>,
}

impl TurnState {
    pub(crate) fn new(
        tool_rounds: usize,
        reads_this_turn: HashSet<String>,
        start_in_post_read_answer_phase: bool,
        pending_runtime_call: Option<PendingRuntimeCall>,
        context_window_tokens: Option<u32>,
    ) -> Self {
        Self {
            tool_rounds,
            reads_this_turn,
            corrections: 0,
            escalation: EngineLocalEscalation::default(),
            last_call_key: None,
            pending_runtime_call,
            search_budget: SearchBudget::new(),
            investigation: InvestigationState::new(),
            turn_perf: TurnPerformance::new(context_window_tokens),
            next_round_label: GenerationRoundLabel::Initial,
            next_round_cause: GenerationRoundCause::Initial,
            requested_read_completed: false,
            read_request_correction_issued: false,
            disallowed_tool_attempts: 0,
            weak_search_query_attempts: 0,
            answer_phase: start_in_post_read_answer_phase.then_some(AnswerPhaseKind::PostRead),
            post_answer_phase_tool_attempts: 0,
            post_answer_phase_correction_echo_retries: 0,
            seeded_tool_executed: false,
            direct_read_result: None,
            answer_guard_retry_entered: false,
            deepening: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_phase_kind_is_copy() {
        let k = AnswerPhaseKind::PostRead;
        let _k2 = k;
        let _k3 = k;
    }

    #[test]
    fn turn_signal_variants_exist() {
        let signals = [
            TurnSignal::Continue,
            TurnSignal::Finish,
            TurnSignal::Suspend,
        ];
        assert_eq!(signals.len(), 3);
    }

    #[test]
    fn engine_local_escalation_defaults_to_zero() {
        let e = EngineLocalEscalation::default();
        assert_eq!(e.closed_search_budget_violations, 0);
        assert_eq!(e.fabricated_tool_result_violations, 0);
        assert_eq!(e.malformed_tool_syntax_violations, 0);
        assert_eq!(e.garbled_edit_repair_violations, 0);
    }
}
