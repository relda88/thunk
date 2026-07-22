use super::super::super::investigation::prompt_analysis::DirectReadMode;
use super::super::super::protocol::response_text::{
    direct_read_fallback_answer, repeated_tool_after_answer_phase_final_answer,
    repeated_tool_after_evidence_ready_final_answer,
};
use super::super::super::types::{AnswerSource, RuntimeEvent, RuntimeTerminalReason};
use super::super::telemetry::{GenerationRoundCause, GenerationRoundLabel};
use super::super::turn_state::{AnswerPhaseKind, TurnContext, TurnSignal, TurnState};
use super::Runtime;

impl Runtime {
    pub(super) fn check_correction_echo(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        response: &str,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<TurnSignal> {
        if let Some(phase) = state.answer_phase {
            // Detect correction echoes by sentinel prefix OR by known correction
            // substrings. The latter catches cases where the model parrots the
            // correction text back without the [runtime:correction] prefix.
            let is_correction_echo = response.trim_start().starts_with("[runtime:correction]")
                || response.contains("The file was already read this turn")
                || response.contains("Evidence is already ready from the file")
                || response.contains("your response does not use it");
            if is_correction_echo {
                self.conversation.discard_last_if_assistant();
                if state.post_answer_phase_correction_echo_retries == 0 {
                    state.post_answer_phase_correction_echo_retries += 1;
                    let (label, cause) = match phase {
                        AnswerPhaseKind::PostRead => (
                            GenerationRoundLabel::CorrectionRetry,
                            GenerationRoundCause::AnswerPhaseToolCallRejected,
                        ),
                        AnswerPhaseKind::InvestigationEvidenceReady => (
                            GenerationRoundLabel::PostEvidenceRetry,
                            GenerationRoundCause::PostEvidenceToolCallRejected,
                        ),
                    };
                    state.next_round_label = label;
                    state.next_round_cause = cause;
                    return Some(TurnSignal::Continue);
                }

                let (answer, reason): (String, RuntimeTerminalReason) = match phase {
                    AnswerPhaseKind::PostRead => {
                        let answer = if matches!(ctx.direct_read_mode, Some(DirectReadMode::Raw)) {
                            state
                                .direct_read_result
                                .as_deref()
                                .map(direct_read_fallback_answer)
                                .unwrap_or_else(|| {
                                    repeated_tool_after_answer_phase_final_answer().to_string()
                                })
                        } else {
                            repeated_tool_after_answer_phase_final_answer().to_string()
                        };
                        (answer, RuntimeTerminalReason::RepeatedToolAfterAnswerPhase)
                    }
                    AnswerPhaseKind::InvestigationEvidenceReady => (
                        repeated_tool_after_evidence_ready_final_answer().to_string(),
                        RuntimeTerminalReason::RepeatedToolAfterEvidenceReady,
                    ),
                };
                self.finish_with_runtime_answer(
                    &answer,
                    AnswerSource::RuntimeTerminal {
                        reason,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return Some(TurnSignal::Finish);
            }
        }
        None
    }
}
