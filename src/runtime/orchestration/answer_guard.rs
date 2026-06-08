use crate::core::config::InvestigationDepth;
use crate::tools::ToolInput;

use super::super::super::investigation::investigation::InvestigationMode;
use super::super::super::paths::{normalize_evidence_path, path_is_within_scope};
use super::super::super::protocol::response_text::*;
use super::super::super::protocol::tool_codec;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{AnswerSource, RuntimeEvent, RuntimeTerminalReason};
use super::super::engine_guards::{extract_claimed_paths, is_definition_only_usage_answer};
use super::super::telemetry::{
    trace_insufficient_evidence_terminal, GenerationRoundCause, GenerationRoundLabel,
};
use super::super::tool_round::{MAX_CANDIDATE_READS_PER_INVESTIGATION, MAX_READS_PER_TURN};
use super::super::turn_state::{PendingRuntimeCall, TurnContext, TurnSignal, TurnState};
use super::Runtime;

impl Runtime {
    // Concerns 2–4: garbled edit repair, fabricated exchange, malformed block
    // (engine.rs:1677–1756)
    pub(super) fn check_protocol_violations(
        &mut self,
        state: &mut TurnState,
        response: &str,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<TurnSignal> {
        // If the previous tool round ended in an edit_file error and the model's repair
        // attempt contains edit_file tag syntax but produced no parseable tool calls,
        // inject a targeted correction rather than silently accepting as Direct.
        if tool_codec::contains_edit_attempt(response)
            && (super::last_injected_was_edit_error(&self.conversation)
                || state.escalation.garbled_edit_repair_violations > 0)
        {
            state.escalation.garbled_edit_repair_violations += 1;
            self.conversation.discard_last_if_assistant();
            if state.escalation.garbled_edit_repair_violations == 1 {
                self.conversation
                    .push_user(EDIT_REPAIR_CORRECTION.to_string());
                state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                state.next_round_cause = GenerationRoundCause::EditRepairCorrection;
                return Some(TurnSignal::Continue);
            }
            self.finish_with_runtime_answer(
                repeated_garbled_edit_repair_final_answer(),
                AnswerSource::RuntimeTerminal {
                    reason: RuntimeTerminalReason::RepeatedGarbledEditRepair,
                    rounds: state.tool_rounds,
                },
                on_event,
            );
            return Some(TurnSignal::Finish);
        }

        // Fabricated [tool_result:] / [tool_error:] blocks mean the model bypassed the
        // protocol. Attempt one automatic correction before surfacing the error.
        if tool_codec::contains_fabricated_exchange(response) {
            state.escalation.fabricated_tool_result_violations += 1;
            self.conversation.discard_last_if_assistant();
            if state.escalation.fabricated_tool_result_violations == 1 {
                self.conversation
                    .push_user(FABRICATION_CORRECTION.to_string());
                state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                state.next_round_cause = GenerationRoundCause::FabricationCorrection;
                return Some(TurnSignal::Continue);
            }
            self.finish_with_runtime_answer(
                repeated_fabricated_tool_result_final_answer(),
                AnswerSource::RuntimeTerminal {
                    reason: RuntimeTerminalReason::RepeatedFabricatedToolResult,
                    rounds: state.tool_rounds,
                },
                on_event,
            );
            return Some(TurnSignal::Finish);
        }

        // Malformed block: a known closing tag ([/write_file], [/edit_file], etc.)
        // is present without the matching opening tag. The model used a wrong tag name.
        // Attempt one correction before giving up.
        if tool_codec::contains_malformed_block(response) {
            state.escalation.malformed_tool_syntax_violations += 1;
            self.conversation.discard_last_if_assistant();
            if state.escalation.malformed_tool_syntax_violations == 1 {
                let correction = match tool_codec::detected_malformed_mutation_tool(response) {
                    Some("edit_file") => malformed_edit_file_correction(),
                    Some("write_file") => malformed_write_file_correction(),
                    _ => MALFORMED_BLOCK_CORRECTION.to_string(),
                };
                self.conversation.push_user(correction);
                state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                state.next_round_cause = GenerationRoundCause::MalformedBlockCorrection;
                return Some(TurnSignal::Continue);
            }
            self.finish_with_runtime_answer(
                repeated_malformed_tool_syntax_final_answer(),
                AnswerSource::RuntimeTerminal {
                    reason: RuntimeTerminalReason::RepeatedMalformedToolSyntax,
                    rounds: state.tool_rounds,
                },
                on_event,
            );
            return Some(TurnSignal::Finish);
        }

        None
    }

    // Concerns 5–9: read request incomplete, empty search terminal,
    // investigation/evidence gates, UsageLookup def-only guard, read-set answer guard
    // (engine.rs:1752–2088)
    pub(super) fn check_evidence_and_admission_gates(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        response: &str,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<TurnSignal> {
        if let Some(path) = ctx.requested_read_path.as_deref() {
            if !state.requested_read_completed {
                if !state.read_request_correction_issued
                    && state.corrections < super::MAX_CORRECTIONS
                {
                    state.corrections += 1;
                    state.read_request_correction_issued = true;
                    self.conversation.push_user(format!(
                        "{READ_REQUEST_TOOL_REQUIRED} Requested path: `{path}`"
                    ));
                    state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                    state.next_round_cause = GenerationRoundCause::ReadRequestToolRequired;
                    return Some(TurnSignal::Continue);
                }

                self.finish_with_runtime_answer(
                    &unread_requested_file_final_answer(path),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::ReadFileFailed,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return Some(TurnSignal::Finish);
            }
        }

        // R4: insufficient-evidence terminal.
        // Search was attempted this turn, all results were empty, and no file
        // was read. The model cannot have any grounded evidence to synthesize from.
        // Discard whatever the model produced and emit the runtime-owned answer.
        if state.search_budget.calls > 0
            && !state.investigation.search_produced_results()
            && state.investigation.files_read_count() == 0
        {
            trace_insufficient_evidence_terminal(
                "empty_search_no_read",
                state.tool_rounds,
                &state.search_budget,
                &state.investigation,
                on_event,
            );
            self.finish_with_runtime_answer(
                insufficient_evidence_final_answer(),
                AnswerSource::RuntimeTerminal {
                    reason: RuntimeTerminalReason::InsufficientEvidence,
                    rounds: state.tool_rounds,
                },
                on_event,
            );
            return Some(TurnSignal::Finish);
        }

        if ctx.investigation_required && !state.investigation.evidence_ready() {
            if state.search_budget.calls == 0 {
                if state.investigation.issue_direct_answer_correction() {
                    self.conversation
                        .push_user(SEARCH_BEFORE_ANSWERING.to_string());
                    state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                    state.next_round_cause = GenerationRoundCause::SearchBeforeAnsweringCorrection;
                    return Some(TurnSignal::Continue);
                }

                trace_insufficient_evidence_terminal(
                    "no_search_after_direct_answer_correction",
                    state.tool_rounds,
                    &state.search_budget,
                    &state.investigation,
                    on_event,
                );
                self.finish_with_runtime_answer(
                    ungrounded_investigation_final_answer(),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return Some(TurnSignal::Finish);
            }

            if state.investigation.search_produced_results() {
                // Shallow mode: terminate after the first candidate read without useful evidence.
                if self.investigation_depth == InvestigationDepth::Shallow
                    && state.investigation.candidate_reads_count() >= 1
                    && !state.investigation.evidence_ready()
                {
                    trace_insufficient_evidence_terminal(
                        "shallow_mode_single_read_exhausted",
                        state.tool_rounds,
                        &state.search_budget,
                        &state.investigation,
                        on_event,
                    );
                    self.finish_with_runtime_answer(
                        ungrounded_investigation_final_answer(),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::InsufficientEvidence,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return Some(TurnSignal::Finish);
                }

                // Both candidate-read slots exhausted and evidence is still not ready.
                // In deep mode: attempt iterative deepening (import-chain following) before
                // terminating. In normal/shallow mode: terminate cleanly.
                if state.investigation.candidate_reads_count()
                    >= MAX_CANDIDATE_READS_PER_INVESTIGATION
                    && !state.investigation.deepening_phase_active
                {
                    if self.investigation_depth == InvestigationDepth::Deep {
                        return Some(self.run_deepening_hop(ctx, state, on_event));
                    }
                    trace_insufficient_evidence_terminal(
                        "candidate_read_limit_exhausted",
                        state.tool_rounds,
                        &state.search_budget,
                        &state.investigation,
                        on_event,
                    );
                    self.finish_with_runtime_answer(
                        ungrounded_investigation_final_answer(),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::InsufficientEvidence,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return Some(TurnSignal::Finish);
                }

                if state.corrections < super::MAX_CORRECTIONS {
                    let candidate = state.investigation.best_unread_candidate_for_mode(
                        ctx.investigation_mode,
                        &state.reads_this_turn,
                    );
                    if let Some(candidate) = candidate {
                        if state.investigation.candidate_reads_count()
                            < MAX_CANDIDATE_READS_PER_INVESTIGATION
                        {
                            if state.investigation.issue_premature_synthesis_correction() {
                                self.conversation.discard_last_if_assistant();
                                state.pending_runtime_call = Some(PendingRuntimeCall {
                                    input: ToolInput::ReadFile { path: candidate },
                                    seeded_pre_generation: false,
                                });
                                state.next_round_label = GenerationRoundLabel::PostTool;
                                state.next_round_cause = GenerationRoundCause::Recovery;
                                return Some(TurnSignal::Continue);
                            }
                            // correction already issued — fall through to text correction or terminal
                        }
                    }
                    if state.investigation.issue_premature_synthesis_correction() {
                        state.corrections += 1;
                        self.conversation.discard_last_if_assistant();
                        self.conversation
                            .push_user(READ_BEFORE_ANSWERING.to_string());
                        state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                        state.next_round_cause =
                            GenerationRoundCause::ReadBeforeAnsweringCorrection;
                        return Some(TurnSignal::Continue);
                    }
                }

                trace_insufficient_evidence_terminal(
                    "read_required_correction_unavailable",
                    state.tool_rounds,
                    &state.search_budget,
                    &state.investigation,
                    on_event,
                );
                self.finish_with_runtime_answer(
                    ungrounded_investigation_final_answer(),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return Some(TurnSignal::Finish);
            }
        }

        // 16.3.2: UsageLookup with definition-only reads.
        if matches!(ctx.investigation_mode, InvestigationMode::UsageLookup)
            && ctx.investigation_required
            && state
                .investigation
                .all_useful_accepted_reads_are_definition_only()
            && (state.investigation.has_non_definition_candidates()
                || is_definition_only_usage_answer(response))
        {
            trace_runtime_decision(
                on_event,
                "terminal_insufficient_evidence",
                &[("reason", "usage_lookup_all_reads_definition_only".into())],
            );
            self.finish_with_runtime_answer(
                insufficient_evidence_final_answer(),
                AnswerSource::RuntimeTerminal {
                    reason: RuntimeTerminalReason::InsufficientEvidence,
                    rounds: state.tool_rounds,
                },
                on_event,
            );
            return Some(TurnSignal::Finish);
        }

        // Read-set answer guard (16.3.1): if the answer text cites a
        // project-looking path that was never successfully read this turn,
        // reject it deterministically rather than surfacing hallucinated evidence.
        // Only fires on state.investigation turns; harmless for direct-read / mutation.
        if ctx.investigation_required && state.investigation.search_produced_results() {
            let claimed = extract_claimed_paths(response);
            if let Some(scope) = ctx.investigation_path_scope.as_deref() {
                if let Some(bad_path) =
                    claimed
                        .iter()
                        .map(|p| normalize_evidence_path(p))
                        .find(|p| {
                            !path_is_within_scope(p, scope)
                                && !state.reads_this_turn.contains(&normalize_evidence_path(
                                    &format!("{}/{p}", scope.trim_end_matches('/')),
                                ))
                        })
                {
                    trace_runtime_decision(
                        on_event,
                        "answer_scope_guard_rejected",
                        &[("path", bad_path.clone()), ("scope", scope.to_string())],
                    );
                    self.finish_with_runtime_answer(
                        &format!(
                            "The investigation is scoped to `{scope}`, but the answer cited \
                             `{bad_path}`. No answer can be given using files outside the \
                             active search scope."
                        ),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::InsufficientEvidence,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return Some(TurnSignal::Finish);
                }
            }
            if let Some(bad_path) = claimed
                .iter()
                .find(|p| !state.reads_this_turn.contains(&normalize_evidence_path(p)))
            {
                let reads_list = {
                    let mut sorted: Vec<&str> =
                        state.reads_this_turn.iter().map(String::as_str).collect();
                    sorted.sort_unstable();
                    sorted.join(",")
                };
                let can_dispatch = !state.answer_guard_retry_entered
                    && state
                        .investigation
                        .is_search_candidate_path(&normalize_evidence_path(bad_path))
                    && state.investigation.candidate_reads_count()
                        < MAX_CANDIDATE_READS_PER_INVESTIGATION
                    && state.reads_this_turn.len() < MAX_READS_PER_TURN;
                if can_dispatch {
                    state.answer_guard_retry_entered = true;
                    self.conversation.discard_last_if_assistant();
                    state.pending_runtime_call = Some(PendingRuntimeCall {
                        input: ToolInput::ReadFile {
                            path: bad_path.clone(),
                        },
                        seeded_pre_generation: false,
                    });
                    state.next_round_label = GenerationRoundLabel::PostTool;
                    state.next_round_cause = GenerationRoundCause::Recovery;
                    return Some(TurnSignal::Continue);
                }
                if !state.answer_guard_retry_entered && !state.reads_this_turn.is_empty() {
                    state.answer_guard_retry_entered = true;
                    trace_runtime_decision(
                        on_event,
                        "answer_guard_rejected",
                        &[
                            ("path", bad_path.clone()),
                            ("reads_count", state.reads_this_turn.len().to_string()),
                            ("reads", reads_list.clone()),
                            (
                                "evidence_ready",
                                state.investigation.evidence_ready().to_string(),
                            ),
                            ("retry_available", "true".to_string()),
                            ("action", "retry".to_string()),
                        ],
                    );
                    self.conversation.discard_last_if_assistant();
                    self.conversation
                        .push_user(answer_guard_retry_constraint(bad_path, &reads_list));
                    state.next_round_label = GenerationRoundLabel::PostEvidenceRetry;
                    state.next_round_cause = GenerationRoundCause::Recovery;
                    return Some(TurnSignal::Continue);
                }
                trace_runtime_decision(
                    on_event,
                    "answer_guard_rejected",
                    &[
                        ("path", bad_path.clone()),
                        ("reads_count", state.reads_this_turn.len().to_string()),
                        ("reads", reads_list),
                        (
                            "evidence_ready",
                            state.investigation.evidence_ready().to_string(),
                        ),
                        ("retry_available", "false".to_string()),
                        ("action", "terminal".to_string()),
                    ],
                );
                self.finish_with_runtime_answer(
                    &format!(
                        "The investigation did not successfully read `{bad_path}` — \
                         this path cannot be cited as evidence. No answer can be given \
                         without reading the relevant file first."
                    ),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
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
