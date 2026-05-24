use std::collections::HashSet;

use crate::app::config::Config;
use crate::llm::backend::ModelBackend;
use crate::tools::{PendingAction, ToolInput, ToolOutput, ToolRegistry, ToolRunResult};

use super::super::conversation::Conversation;
use super::super::investigation::anchors::{
    has_same_scope_reference, is_last_read_file_anchor_prompt, is_last_search_anchor_prompt,
    AnchorState,
};
use super::super::investigation::investigation::{
    detect_investigation_mode, InvestigationMode,
};
use super::super::paths::{normalize_evidence_path, path_is_within_scope};
use super::super::project::ProjectRoot;
use super::super::project::ProjectStructureSnapshot;
use super::super::project::ProjectStructureSnapshotCache;
use super::super::protocol::prompt;
use super::super::protocol::tool_codec;
use super::super::resolve;
use super::super::types::{
    Activity, AnswerSource, RuntimeEvent, RuntimeRequest, RuntimeTerminalReason,
};
use super::context_policy::ContextPolicy;
use super::generation::{emit_visible_assistant_message, run_generate_turn};
use super::tool_round::{
    run_tool_round, ToolRoundOutcome, MAX_CANDIDATE_READS_PER_INVESTIGATION,
    MAX_READS_PER_TURN,
};

#[path = "anchor_resolution.rs"]
mod anchor_resolution;

#[path = "command_handlers.rs"]
mod command_handlers;

/// Maximum tool rounds per turn. Prevents runaway loops when the model keeps
/// producing tool calls without reaching a final answer.
const MAX_TOOL_ROUNDS: usize = 10;

/// Maximum automatic corrections per turn. One correction is enough — if the
/// model fabricates twice in a row the prompt fix is insufficient and we surface
/// the failure rather than looping silently.
const MAX_CORRECTIONS: usize = 1;

use super::super::protocol::response_text::*;
use super::super::trace::trace_runtime_decision;
use super::context_cap::{cap_tool_result_blocks, estimate_generation_prompt_chars};
use super::engine_guards::{extract_claimed_paths, is_definition_only_usage_answer, usage_lookup_is_broad};
use super::telemetry::{
    infer_post_tool_round_cause, short_tool_name, tool_input_activity,
    trace_insufficient_evidence_terminal, GenerationRoundCause, GenerationRoundLabel,
};

use super::super::investigation::tool_surface::{select_tool_surface, ToolSurface};

use super::turn_state::{
    AnswerPhaseKind, PendingRuntimeCall, TurnContext, TurnSignal, TurnState,
};

/// Returns true if the prompt contains a token that looks like a code identifier.
/// Only two structural patterns are checked — no NLP, no heuristics.
use super::super::investigation::prompt_analysis::{
    classify_retrieval_intent, extract_investigation_path_scope, prompt_requires_investigation,
    is_permitted_shell_command, requested_shell_command, requested_simple_edit,
    user_requested_execution, user_requested_mutation, DirectReadMode, RetrievalIntent,
};

pub struct Runtime {
    #[allow(dead_code)]
    project_root: ProjectRoot,
    conversation: Conversation,
    backend: Box<dyn ModelBackend>,
    registry: ToolRegistry,
    system_prompt: String,
    pub(crate) anchors: AnchorState,
    context_policy: ContextPolicy,
    project_snapshot_cache: ProjectStructureSnapshotCache,
    /// Holds a mutating tool action that is waiting for user approval.
    /// Set when a tool round suspends; cleared by Approve or Reject.
    /// At most one pending action exists at any time.
    pending_action: Option<PendingAction>,
    config: Config,
    /// Queued runtime-owned tool call to execute at the start of the next run_turns invocation.
    /// Set by handle_approve when a post-mutation follow-up (e.g. test run) is configured.
    pending_runtime_call: Option<PendingRuntimeCall>,
    /// Per-session undo stack. Each entry is (absolute_path, before_contents).
    /// Empty string for before_contents means the file did not exist before write_file created it.
    /// Capped at 5 entries — oldest dropped when exceeded.
    undo_stack: Vec<(String, String)>,
}

impl Runtime {
    pub fn new(
        config: &Config,
        project_root: ProjectRoot,
        backend: Box<dyn ModelBackend>,
        registry: ToolRegistry,
    ) -> Self {
        let specs = registry.specs();
        let system_prompt = prompt::build_system_prompt(
            &config.app.name,
            project_root.path(),
            &specs,
            false,
        );
        let context_policy = ContextPolicy::from_capabilities(backend.capabilities());
        Self {
            project_root,
            conversation: Conversation::new(system_prompt.clone()),
            backend,
            registry,
            system_prompt,
            anchors: AnchorState::default(),
            context_policy,
            project_snapshot_cache: ProjectStructureSnapshotCache::default(),
            pending_action: None,
            config: config.clone(),
            pending_runtime_call: None,
            undo_stack: Vec::new(),
        }
    }

    /// Returns a snapshot of all current conversation messages for persistence.
    pub fn messages_snapshot(&self) -> Vec<crate::llm::backend::Message> {
        self.conversation.snapshot()
    }

    /// Appends historical messages into the conversation after the system prompt.
    /// Called once at startup when restoring a prior session. Not for use mid-turn.
    pub fn load_history(&mut self, messages: Vec<crate::llm::backend::Message>) {
        self.conversation.extend_history(messages);
    }

    /// Restores anchor state persisted from a prior session.
    /// Called once at startup after session restore, parallel to load_history.
    /// Uses the existing anchor update mechanism so invariants are preserved.
    pub fn restore_anchors(
        &mut self,
        last_read_file: Option<String>,
        last_search_query: Option<String>,
        last_search_scope: Option<String>,
    ) {
        if let Some(path) = last_read_file {
            let output =
                crate::tools::ToolOutput::FileContents(crate::tools::types::FileContentsOutput {
                    path,
                    contents: String::new(),
                    total_lines: 0,
                    truncated: false,
                });
            self.anchors.record_successful_read(&output);
        }
        if let Some(query) = last_search_query {
            let output =
                crate::tools::ToolOutput::SearchResults(crate::tools::types::SearchResultsOutput {
                    query: query.clone(),
                    matches: vec![],
                    total_matches: 0,
                    truncated: false,
                });
            self.anchors
                .record_successful_search(&output, query, last_search_scope);
        }
    }

    /// Returns a snapshot of the current anchor state for persistence.
    pub fn anchors_snapshot(&self) -> (Option<String>, Option<String>, Option<String>) {
        let last_read_file = self.anchors.last_read_file().map(str::to_string);
        let (last_search_query, last_search_scope) = match self.anchors.last_search() {
            Some((q, s)) => (Some(q), s),
            None => (None, None),
        };
        (last_read_file, last_search_query, last_search_scope)
    }

    /// Handles a RuntimeRequest by updating the conversation, invoking the backend,
    /// and firing RuntimeEvents to drive the UI. Each request type has its own
    /// handler method for clarity.
    pub fn handle(&mut self, request: RuntimeRequest, on_event: &mut dyn FnMut(RuntimeEvent)) {
        match request {
            RuntimeRequest::Submit { text } => self.handle_submit(text, on_event),
            RuntimeRequest::Reset => self.handle_reset(on_event),
            RuntimeRequest::Approve => self.handle_approve(on_event),
            RuntimeRequest::Reject => self.handle_reject(on_event),
            RuntimeRequest::QueryLast => self.handle_query_last(on_event),
            RuntimeRequest::QueryAnchors => self.handle_query_anchors(on_event),
            RuntimeRequest::QueryHistory => self.handle_query_history(on_event),
            RuntimeRequest::ReadFile { path } => self.handle_read_file(path, on_event),
            RuntimeRequest::SearchCode { query } => self.handle_search_code(query, on_event),
            RuntimeRequest::Undo => self.handle_undo(on_event),
            RuntimeRequest::ProvidersList => self.handle_providers_list(on_event),
            RuntimeRequest::ProvidersUse { name } => self.handle_providers_use(name, on_event),
        }
    }

    /// Applies the Layer 1 context cap then commits the results to the conversation.
    /// Must be used for all tool-origin push_user calls so the cap is applied consistently.
    fn commit_tool_results(&mut self, results: String) {
        let capped = cap_tool_result_blocks(&results, self.context_policy.tool_result_max_lines);
        self.conversation.push_user(capped);
    }

    fn get_or_build_project_snapshot(&mut self) -> std::io::Result<&ProjectStructureSnapshot> {
        self.project_snapshot_cache.get_or_build(&self.project_root)
    }

    fn maybe_render_project_snapshot_hint(&mut self, tool_surface: ToolSurface) -> Option<String> {
        if !tool_surface.includes_project_snapshot_hint() {
            return None;
        }

        let snapshot = self.get_or_build_project_snapshot().ok()?;
        Some(prompt::render_project_snapshot_hint(snapshot))
    }

    fn invalidate_project_snapshot(&mut self) {
        self.project_snapshot_cache.invalidate();
    }

    fn invalidate_project_snapshot_if_needed(&mut self, output: &ToolOutput) {
        if matches!(
            output,
            ToolOutput::WriteFile(_) | ToolOutput::EditFile(_) | ToolOutput::Shell(_)
        ) {
            self.invalidate_project_snapshot();
        }
    }

    fn handle_submit(&mut self, text: String, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::Failed {
                message:
                    "Cannot submit while a tool approval is pending. Use /approve or /reject first."
                        .to_string(),
            });
            return;
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            on_event(RuntimeEvent::Failed {
                message: "Cannot submit an empty prompt.".to_string(),
            });
            return;
        }

        let is_last_read_file_anchor = is_last_read_file_anchor_prompt(trimmed);
        let is_last_search_anchor = is_last_search_anchor_prompt(trimmed);
        self.conversation.push_user(text);
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        if is_last_read_file_anchor {
            trace_runtime_decision(
                on_event,
                "anchor_prompt_matched",
                &[("kind", "last_read_file".into())],
            );
            if let Some(path) = self.anchors.last_read_file().map(str::to_string) {
                trace_runtime_decision(
                    on_event,
                    "anchor_resolved",
                    &[("kind", "last_read_file".into()), ("path", path.clone())],
                );
                self.run_last_read_file_anchor(path, on_event);
            } else {
                trace_runtime_decision(
                    on_event,
                    "anchor_missing",
                    &[("kind", "last_read_file".into())],
                );
                self.finish_with_runtime_answer(
                    NO_LAST_READ_FILE_AVAILABLE,
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::ReadFileFailed,
                        rounds: 0,
                    },
                    on_event,
                );
            }
            return;
        }
        if is_last_search_anchor {
            trace_runtime_decision(
                on_event,
                "anchor_prompt_matched",
                &[("kind", "last_search".into())],
            );
            if let Some((query, scope)) = self.anchors.last_search() {
                trace_runtime_decision(
                    on_event,
                    "anchor_resolved",
                    &[
                        ("kind", "last_search".into()),
                        ("query", query.clone()),
                        ("scope", scope.clone().unwrap_or_else(|| "none".into())),
                    ],
                );
                self.run_last_search_anchor(query, scope, on_event);
            } else {
                trace_runtime_decision(
                    on_event,
                    "anchor_missing",
                    &[("kind", "last_search".into())],
                );
                self.finish_with_runtime_answer(
                    NO_LAST_SEARCH_AVAILABLE,
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: 0,
                    },
                    on_event,
                );
            }
            return;
        }
        self.run_turns(0, on_event);
    }

    fn handle_approve(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let pending = match self.pending_action.take() {
            Some(p) => p,
            None => {
                on_event(RuntimeEvent::Failed {
                    message: "No pending action to approve.".to_string(),
                });
                return;
            }
        };

        let tool_name = pending.tool_name.clone();
        on_event(RuntimeEvent::ActivityChanged(Activity::ExecutingTools {
            tool: short_tool_name(&tool_name).to_string(),
            detail: None,
        }));

        if matches!(tool_name.as_str(), "edit_file" | "write_file") {
            if let Some(abs_path) = extract_absolute_path_from_payload(&pending.payload) {
                let before = std::fs::read_to_string(&abs_path).unwrap_or_default();
                self.undo_stack.push((abs_path, before));
                if self.undo_stack.len() > 5 {
                    self.undo_stack.remove(0);
                }
            }
        }

        match self.registry.execute_approved(&pending) {
            Ok(output) => {
                self.invalidate_project_snapshot_if_needed(&output);
                let summary = tool_codec::render_compact_summary(&output);
                let final_answer = mutation_complete_final_answer(&tool_name, &summary);
                on_event(RuntimeEvent::ToolCallFinished {
                    name: tool_name.clone(),
                    summary: Some(summary),
                });
                self.commit_tool_results(tool_codec::format_tool_result(&tool_name, &output));
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    &final_answer,
                    AnswerSource::ToolAssisted { rounds: 1 },
                    on_event,
                );
                if matches!(tool_name.as_str(), "edit_file" | "write_file") {
                    let test_cmd = self.config.project.test_command.clone();
                    if let Some(cmd) = test_cmd {
                        let input = ToolInput::Shell { command: cmd };
                        if let Ok(resolved) = resolve(&self.project_root, &input) {
                            match self.registry.dispatch(resolved) {
                                Ok(ToolRunResult::Approval(pending)) => {
                                    self.pending_action = Some(pending.clone());
                                    on_event(RuntimeEvent::ApprovalRequired { pending, evidence: vec![] });
                                }
                                Ok(ToolRunResult::Immediate(output)) => {
                                    self.invalidate_project_snapshot_if_needed(&output);
                                    self.commit_tool_results(
                                        tool_codec::format_tool_result("shell", &output),
                                    );
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }
            }
            Err(e) => {
                on_event(RuntimeEvent::ToolCallFinished {
                    name: tool_name.clone(),
                    summary: None,
                });
                let error_text = tool_codec::format_tool_error(&tool_name, &e.to_string());
                self.conversation.push_user(error_text);
                // On failure, let the model respond — it may want to retry.
                on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                self.run_turns(0, on_event);
            }
        }
    }

    fn handle_reject(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let pending = match self.pending_action.take() {
            Some(p) => p,
            None => {
                on_event(RuntimeEvent::Failed {
                    message: "No pending action to reject.".to_string(),
                });
                return;
            }
        };

        let tool_name = pending.tool_name.clone();
        on_event(RuntimeEvent::ToolCallFinished {
            name: tool_name.clone(),
            summary: None,
        });
        let rejection = tool_codec::format_tool_error(
            &tool_name,
            "user rejected this action — do not retry or re-propose it. \
             Acknowledge the cancellation in plain text and wait for the user's next instruction.",
        );
        self.conversation.push_user(rejection);
        self.finish_with_runtime_answer(
            rejection_final_answer(&tool_name),
            AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::RejectedMutation,
                rounds: 1,
            },
            on_event,
        );
    }

    /// Runs the generate -> tool-round loop until the model produces a final answer,
    /// the tool round limit is reached, or a tool action requires approval.
    /// `tool_rounds` is the count already consumed before this call (0 for a fresh turn).
    fn run_turns(&mut self, tool_rounds: usize, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.run_turns_with_initial_reads(tool_rounds, HashSet::new(), false, on_event);
    }

    fn run_turns_with_initial_reads(
        &mut self,
        tool_rounds: usize,
        reads_this_turn: HashSet<String>,
        start_in_post_read_answer_phase: bool,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let Ok(ctx) =
            TurnContext::build(self, tool_rounds, &reads_this_turn, on_event)
        else {
            return;
        };
        let mut state = TurnState::new(
            tool_rounds,
            reads_this_turn,
            start_in_post_read_answer_phase,
            self.pending_runtime_call.take(),
            self.backend.capabilities().context_window_tokens,
        );
        seed_pending_runtime_call(&ctx, &mut state);
        loop {
            match self.run_loop_body(&ctx, &mut state, on_event) {
                TurnSignal::Finish => {
                    state.turn_perf.emit_summary(on_event);
                    return;
                }
                TurnSignal::Continue => continue,
                TurnSignal::Suspend => return,
            }
        }
    }

    fn run_loop_body(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> TurnSignal {
        let effective_surface = if state.answer_phase.is_some() {
            ToolSurface::AnswerOnly
        } else {
            ctx.tool_surface
        };
            if matches!(effective_surface, ToolSurface::AnswerOnly) {
                trace_runtime_decision(
                    on_event,
                    "answer_phase_synthesis_bounded",
                    &[("surface", "AnswerOnly".into())],
                );
            }
            let is_correction_round = !matches!(
                state.next_round_cause,
                GenerationRoundCause::Initial
                    | GenerationRoundCause::ToolResults
                    | GenerationRoundCause::ReadRequestToolRequired
                    | GenerationRoundCause::ReadBeforeAnsweringCorrection
            );
            let project_snapshot_hint = if state.pending_runtime_call.is_none() && !is_correction_round {
                self.maybe_render_project_snapshot_hint(effective_surface)
            } else {
                None
            };
            let prompt_chars = if state.turn_perf.is_enabled() {
                estimate_generation_prompt_chars(
                    &self.conversation,
                    effective_surface,
                    project_snapshot_hint.as_deref(),
                )
            } else {
                0
            };

            state.turn_perf.start_round(state.next_round_label, state.next_round_cause, prompt_chars, on_event);

            let (calls, response, seeded_pre_generation) = if let Some(pending) =
                state.pending_runtime_call.take()
            {
                (vec![pending.input], None, pending.seeded_pre_generation)
            } else {
                let response = {
                    let mut perf_on_event = |event| {
                        if let RuntimeEvent::BackendTiming { stage, elapsed_ms } = &event {
                            state.turn_perf.record_backend_timing(*stage, *elapsed_ms);
                        }
                        if let RuntimeEvent::BackendTokenCounts { prompt, completion } = &event {
                            state.turn_perf.record_token_counts(*prompt, *completion);
                        }
                        on_event(event);
                    };

                    match run_generate_turn(
                        self.backend.as_mut(),
                        &mut self.conversation,
                        effective_surface,
                        project_snapshot_hint.as_deref(),
                        ctx.investigation_mode,
                        &mut perf_on_event,
                    ) {
                        Ok(Some(r)) => r,
                        Ok(None) => {
                            on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                            on_event(RuntimeEvent::Failed {
                                message: format!("{} returned no output.", self.backend.name()),
                            });
                            return TurnSignal::Finish;
                        }
                        Err(e) => {
                            on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                            on_event(RuntimeEvent::Failed {
                                message: e.to_string(),
                            });
                            return TurnSignal::Finish;
                        }
                    }
                };

                let calls = tool_codec::parse_all_tool_inputs(&response);
                (calls, Some(response), false)
            };

            if let Some(phase) = state.answer_phase {
                if !calls.is_empty() && response.is_some() {
                    state.post_answer_phase_tool_attempts += 1;
                    if matches!(phase, AnswerPhaseKind::InvestigationEvidenceReady) {
                        trace_runtime_decision(
                            on_event,
                            "post_evidence_tool_call_rejected",
                            &[
                                ("attempts", state.post_answer_phase_tool_attempts.to_string()),
                                ("tool_count", calls.len().to_string()),
                            ],
                        );
                    }
                    self.conversation.discard_last_if_assistant();
                    if state.post_answer_phase_tool_attempts == 1 {
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
                        self.conversation.push_user(
                            match phase {
                                AnswerPhaseKind::PostRead => TURN_COMPLETE_ANSWER_ONLY,
                                AnswerPhaseKind::InvestigationEvidenceReady => {
                                    EVIDENCE_READY_ANSWER_ONLY
                                }
                            }
                            .to_string(),
                        );
                        return TurnSignal::Continue;
                    }
                    let (answer, reason): (String, RuntimeTerminalReason) = match phase {
                        AnswerPhaseKind::PostRead => {
                            let answer = if matches!(ctx.direct_read_mode, Some(DirectReadMode::Raw)) {
                                state.direct_read_result
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
                    return TurnSignal::Finish;
                }
            }

            if state.search_budget.is_closed()
                && calls
                    .iter()
                    .any(|c| matches!(c, ToolInput::SearchCode { .. }))
            {
                if state.search_budget.empty_retry_exhausted()
                    && !state.investigation.search_produced_results()
                    && state.investigation.files_read_count() == 0
                {
                    trace_insufficient_evidence_terminal(
                        "empty_search_retry_exhausted",
                        state.tool_rounds,
                        &state.search_budget,
                        &state.investigation,
                        on_event,
                    );
                    self.conversation.discard_last_if_assistant();
                    self.finish_with_runtime_answer(
                        insufficient_evidence_final_answer(),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::InsufficientEvidence,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return TurnSignal::Finish;
                }
                state.escalation.closed_search_budget_violations += 1;
                self.conversation.discard_last_if_assistant();
                if state.escalation.closed_search_budget_violations == 1 {
                    self.conversation
                        .push_user(state.search_budget.closed_message().to_string());
                    state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                    state.next_round_cause = GenerationRoundCause::SearchBudgetClosedCorrection;
                    return TurnSignal::Continue;
                }
                self.finish_with_runtime_answer(
                    repeated_search_budget_violation_final_answer(),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::RepeatedSearchBudgetViolation,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return TurnSignal::Finish;
            }

            if calls.is_empty() {
                let response = response.expect("response exists when calls are empty");

                if let Some(phase) = state.answer_phase {
                    // Detect correction echoes by sentinel prefix OR by known correction
                    // substrings. The latter catches cases where the model parrots the
                    // correction text back without the [runtime:correction] prefix.
                    let is_correction_echo =
                        response.trim_start().starts_with("[runtime:correction]")
                            || response.contains("The file was already read this turn")
                            || response.contains("Evidence is already ready from the file");
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
                            return TurnSignal::Continue;
                        }

                        let (answer, reason): (String, RuntimeTerminalReason) = match phase {
                            AnswerPhaseKind::PostRead => {
                                let answer =
                                    if matches!(ctx.direct_read_mode, Some(DirectReadMode::Raw)) {
                                        state.direct_read_result
                                            .as_deref()
                                            .map(direct_read_fallback_answer)
                                            .unwrap_or_else(|| {
                                                repeated_tool_after_answer_phase_final_answer()
                                                    .to_string()
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
                        return TurnSignal::Finish;
                    }
                }

                // If the previous tool round ended in an edit_file error and the model's repair
                // attempt contains edit_file tag syntax but produced no parseable tool calls,
                // inject a targeted correction rather than silently accepting as Direct.
                if tool_codec::contains_edit_attempt(&response)
                    && (last_injected_was_edit_error(&self.conversation)
                        || state.escalation.garbled_edit_repair_violations > 0)
                {
                    state.escalation.garbled_edit_repair_violations += 1;
                    self.conversation.discard_last_if_assistant();
                    if state.escalation.garbled_edit_repair_violations == 1 {
                        self.conversation
                            .push_user(EDIT_REPAIR_CORRECTION.to_string());
                        state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                        state.next_round_cause = GenerationRoundCause::EditRepairCorrection;
                        return TurnSignal::Continue;
                    }
                    self.finish_with_runtime_answer(
                        repeated_garbled_edit_repair_final_answer(),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::RepeatedGarbledEditRepair,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return TurnSignal::Finish;
                }

                // Fabricated [tool_result:] / [tool_error:] blocks mean the model bypassed the
                // protocol. Attempt one automatic correction before surfacing the error.
                if tool_codec::contains_fabricated_exchange(&response) {
                    state.escalation.fabricated_tool_result_violations += 1;
                    self.conversation.discard_last_if_assistant();
                    if state.escalation.fabricated_tool_result_violations == 1 {
                        self.conversation
                            .push_user(FABRICATION_CORRECTION.to_string());
                        state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                        state.next_round_cause = GenerationRoundCause::FabricationCorrection;
                        return TurnSignal::Continue;
                    }
                    self.finish_with_runtime_answer(
                        repeated_fabricated_tool_result_final_answer(),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::RepeatedFabricatedToolResult,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return TurnSignal::Finish;
                }
                // Malformed block: a known closing tag ([/write_file], [/edit_file], etc.)
                // is present without the matching opening tag. The model used a wrong tag name.
                // Attempt one correction before giving up.
                if tool_codec::contains_malformed_block(&response) {
                    state.escalation.malformed_tool_syntax_violations += 1;
                    self.conversation.discard_last_if_assistant();
                    if state.escalation.malformed_tool_syntax_violations == 1 {
                        let correction =
                            match tool_codec::detected_malformed_mutation_tool(&response) {
                                Some("edit_file") => malformed_edit_file_correction(),
                                Some("write_file") => malformed_write_file_correction(),
                                _ => MALFORMED_BLOCK_CORRECTION.to_string(),
                            };
                        self.conversation.push_user(correction);
                        state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                        state.next_round_cause = GenerationRoundCause::MalformedBlockCorrection;
                        return TurnSignal::Continue;
                    }
                    self.finish_with_runtime_answer(
                        repeated_malformed_tool_syntax_final_answer(),
                        AnswerSource::RuntimeTerminal {
                            reason: RuntimeTerminalReason::RepeatedMalformedToolSyntax,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return TurnSignal::Finish;
                }

                if let Some(path) = ctx.requested_read_path.as_deref() {
                    if !state.requested_read_completed {
                        if !state.read_request_correction_issued && state.corrections < MAX_CORRECTIONS {
                            state.corrections += 1;
                            state.read_request_correction_issued = true;
                            self.conversation.push_user(format!(
                                "{READ_REQUEST_TOOL_REQUIRED} Requested path: `{path}`"
                            ));
                            state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                            state.next_round_cause = GenerationRoundCause::ReadRequestToolRequired;
                            return TurnSignal::Continue;
                        }

                        self.finish_with_runtime_answer(
                            &unread_requested_file_final_answer(path),
                            AnswerSource::RuntimeTerminal {
                                reason: RuntimeTerminalReason::ReadFileFailed,
                                rounds: state.tool_rounds,
                            },
                            on_event,
                        );
                        return TurnSignal::Finish;
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
                    return TurnSignal::Finish;
                }

                if ctx.investigation_required && !state.investigation.evidence_ready() {
                    if state.search_budget.calls == 0 {
                        if state.investigation.issue_direct_answer_correction() {
                            self.conversation
                                .push_user(SEARCH_BEFORE_ANSWERING.to_string());
                            state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                            state.next_round_cause =
                                GenerationRoundCause::SearchBeforeAnsweringCorrection;
                            return TurnSignal::Continue;
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
                        return TurnSignal::Finish;
                    }

                    if state.investigation.search_produced_results() {
                        // Both candidate-read slots exhausted and evidence is still not ready.
                        // Do not attempt another correction cycle — terminate cleanly.
                        if state.investigation.candidate_reads_count()
                            >= MAX_CANDIDATE_READS_PER_INVESTIGATION
                        {
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
                            return TurnSignal::Finish;
                        }

                        if state.corrections < MAX_CORRECTIONS {
                            let candidate = state.investigation
                                .best_candidate_for_mode(ctx.investigation_mode)
                                .map(str::to_string);
                            if let Some(candidate) = candidate {
                                if state.investigation.candidate_reads_count()
                                    < MAX_CANDIDATE_READS_PER_INVESTIGATION
                                {
                                    self.conversation.discard_last_if_assistant();
                                    state.investigation.issue_premature_synthesis_correction();
                                    state.pending_runtime_call = Some(PendingRuntimeCall {
                                        input: ToolInput::ReadFile { path: candidate },
                                        seeded_pre_generation: false,
                                    });
                                    state.next_round_label = GenerationRoundLabel::PostTool;
                                    state.next_round_cause = GenerationRoundCause::Recovery;
                                    return TurnSignal::Continue;
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
                                return TurnSignal::Continue;
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
                        return TurnSignal::Finish;
                    }
                }

                // 16.3.2: UsageLookup with definition-only reads.
                if matches!(ctx.investigation_mode, InvestigationMode::UsageLookup)
                    && ctx.investigation_required
                    && state.investigation.all_useful_accepted_reads_are_definition_only()
                    && (state.investigation.has_non_definition_candidates()
                        || is_definition_only_usage_answer(&response))
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
                    return TurnSignal::Finish;
                }

                // Read-set answer guard (16.3.1): if the answer text cites a
                // project-looking path that was never successfully read this turn,
                // reject it deterministically rather than surfacing hallucinated evidence.
                // Only fires on state.investigation turns; harmless for direct-read / mutation.
                if ctx.investigation_required && state.investigation.search_produced_results() {
                    let claimed = extract_claimed_paths(&response);
                    if let Some(scope) = ctx.investigation_path_scope.as_deref() {
                        if let Some(bad_path) = claimed
                            .iter()
                            .map(|p| normalize_evidence_path(p))
                            .find(|p| !path_is_within_scope(p, scope))
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
                            return TurnSignal::Finish;
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
                            && !state.investigation.evidence_ready()
                            && state.investigation
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
                            return TurnSignal::Continue;
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
                                    ("evidence_ready", state.investigation.evidence_ready().to_string()),
                                    ("retry_available", "true".to_string()),
                                    ("action", "retry".to_string()),
                                ],
                            );
                            self.conversation.discard_last_if_assistant();
                            self.conversation
                                .push_user(answer_guard_retry_constraint(bad_path, &reads_list));
                            state.next_round_label = GenerationRoundLabel::PostEvidenceRetry;
                            state.next_round_cause = GenerationRoundCause::Recovery;
                            return TurnSignal::Continue;
                        }
                        trace_runtime_decision(
                            on_event,
                            "answer_guard_rejected",
                            &[
                                ("path", bad_path.clone()),
                                ("reads_count", state.reads_this_turn.len().to_string()),
                                ("reads", reads_list),
                                ("evidence_ready", state.investigation.evidence_ready().to_string()),
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
                        return TurnSignal::Finish;
                    }
                }

                let source = if state.tool_rounds == 0 {
                    if state.seeded_tool_executed {
                        AnswerSource::ToolAssisted { rounds: 1 }
                    } else {
                        AnswerSource::Direct
                    }
                } else {
                    AnswerSource::ToolAssisted {
                        rounds: state.tool_rounds,
                    }
                };
                emit_visible_assistant_message(&response, on_event);
                on_event(RuntimeEvent::AnswerReady(source));
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                return TurnSignal::Finish;
            }

            if !seeded_pre_generation {
                state.tool_rounds += 1;

                if state.tool_rounds >= MAX_TOOL_ROUNDS {
                    on_event(RuntimeEvent::AnswerReady(AnswerSource::ToolLimitReached));
                    on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                    return TurnSignal::Finish;
                }
            }

            on_event(RuntimeEvent::ActivityChanged(tool_input_activity(calls.first())));
            let t_tool_start = if state.turn_perf.is_enabled() {
                Some(std::time::Instant::now())
            } else {
                None
            };

            match run_tool_round(
                &self.project_root,
                &self.registry,
                calls,
                &mut state.last_call_key,
                &mut state.search_budget,
                &mut state.investigation,
                &mut state.reads_this_turn,
                &mut self.anchors,
                ctx.tool_surface,
                &mut state.disallowed_tool_attempts,
                &mut state.weak_search_query_attempts,
                ctx.mutation_allowed,
                ctx.investigation_required,
                ctx.investigation_mode,
                ctx.requested_read_path.as_deref(),
                &mut state.requested_read_completed,
                ctx.investigation_path_scope.as_deref(),
                on_event,
            ) {
                ToolRoundOutcome::Completed {
                    results,
                    git_acquisition_answer,
                } => {
                    if seeded_pre_generation {
                        state.seeded_tool_executed = true;
                        state.last_call_key = None;
                        if matches!(ctx.retrieval_intent, RetrievalIntent::DirectoryListing { .. }) {
                            state.answer_phase = Some(AnswerPhaseKind::PostRead);
                        }
                        // Invariant: ctx.requested_read_path.is_some() identifies a DirectRead turn.
                        // Capture the result now (before commit moves it) so the runtime can
                        // serve it as a deterministic fallback if model synthesis loops.
                        if ctx.requested_read_path.is_some() {
                            state.direct_read_result = Some(results.clone());
                            if matches!(ctx.direct_read_mode, Some(DirectReadMode::Explain)) {
                                state.answer_phase = Some(AnswerPhaseKind::PostRead);
                            }
                        }
                    }
                    if let Some(t) = t_tool_start {
                        state.turn_perf.record_tool_elapsed(t.elapsed().as_millis() as u64);
                    }
                    if seeded_pre_generation
                        && matches!(ctx.direct_read_mode, Some(DirectReadMode::Raw))
                    {
                        let answer = direct_read_fallback_answer(&results);
                        self.commit_tool_results(results);
                        self.conversation
                            .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                        self.finish_with_runtime_answer(
                            &answer,
                            AnswerSource::ToolAssisted { rounds: 1 },
                            on_event,
                        );
                        return TurnSignal::Finish;
                    }
                    let post_tool_cause = infer_post_tool_round_cause(&results);
                    self.commit_tool_results(results);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                    if ctx.tool_surface == ToolSurface::GitReadOnly {
                        if let Some(answer) = git_acquisition_answer {
                            trace_runtime_decision(
                                on_event,
                                "git_acquisition_completed",
                                &[("rounds", state.tool_rounds.to_string())],
                            );
                            self.finish_with_runtime_answer(
                                &answer,
                                AnswerSource::ToolAssisted {
                                    rounds: state.tool_rounds,
                                },
                                on_event,
                            );
                            return TurnSignal::Finish;
                        }
                    }
                    if state.answer_phase.is_none() {
                        if ctx.investigation_required && state.investigation.evidence_ready() {
                            state.answer_phase = Some(AnswerPhaseKind::InvestigationEvidenceReady);
                        } else if !ctx.investigation_required
                            && !ctx.mutation_allowed
                            && !state.reads_this_turn.is_empty()
                        {
                            state.answer_phase = Some(AnswerPhaseKind::PostRead);
                        }
                    }
                    state.next_round_label = GenerationRoundLabel::PostTool;
                    state.next_round_cause = post_tool_cause;
                    // Signal re-entry before the next generate so the status bar
                    // transitions cleanly from "executing tools" → "processing" → …
                    on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                    // Do not return — loop continues so the model is re-invoked
                    // with the tool results in context to produce a synthesis response.
                }
                ToolRoundOutcome::TerminalAnswer {
                    results,
                    answer,
                    reason,
                } => {
                    if let Some(t) = t_tool_start {
                        state.turn_perf.record_tool_elapsed(t.elapsed().as_millis() as u64);
                    }
                    self.commit_tool_results(results);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                    self.finish_with_runtime_answer(
                        &answer,
                        AnswerSource::RuntimeTerminal {
                            reason,
                            rounds: state.tool_rounds,
                        },
                        on_event,
                    );
                    return TurnSignal::Finish;
                }
                ToolRoundOutcome::ApprovalRequired {
                    accumulated,
                    pending,
                } => {
                    if let Some(t) = t_tool_start {
                        state.turn_perf.record_tool_elapsed(t.elapsed().as_millis() as u64);
                    }
                    if !accumulated.is_empty() {
                        self.commit_tool_results(accumulated);
                        self.conversation
                            .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                    }
                    self.pending_action = Some(pending.clone());
                    let evidence = state.investigation.evidence_summary();
                    on_event(RuntimeEvent::ApprovalRequired { pending, evidence });
                    on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                    return TurnSignal::Finish;
                }
                ToolRoundOutcome::RuntimeDispatch { accumulated, call } => {
                    if let Some(t) = t_tool_start {
                        state.turn_perf.record_tool_elapsed(t.elapsed().as_millis() as u64);
                    }
                    if !accumulated.is_empty() {
                        self.commit_tool_results(accumulated);
                        self.conversation
                            .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                    }
                    state.pending_runtime_call = Some(PendingRuntimeCall {
                        input: call,
                        seeded_pre_generation: false,
                    });
                    on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                }
            }
        TurnSignal::Continue
    }

    fn finish_with_runtime_answer(
        &mut self,
        answer: &str,
        source: AnswerSource,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        on_event(RuntimeEvent::ActivityChanged(Activity::Responding));
        self.conversation.begin_assistant_reply();
        on_event(RuntimeEvent::AssistantMessageStarted);
        self.conversation.push_assistant_chunk(answer);
        on_event(RuntimeEvent::AssistantMessageChunk(answer.to_string()));
        on_event(RuntimeEvent::AssistantMessageFinished);
        on_event(RuntimeEvent::AnswerReady(source));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    #[cfg(test)]
    pub(crate) fn set_pending_for_test(&mut self, action: PendingAction) {
        self.pending_action = Some(action);
    }

    #[cfg(test)]
    pub(crate) fn project_snapshot_for_test(
        &mut self,
    ) -> std::io::Result<ProjectStructureSnapshot> {
        self.get_or_build_project_snapshot().cloned()
    }
}

impl TurnContext {
    fn build(
        runtime: &mut Runtime,
        tool_rounds: usize,
        reads_this_turn: &HashSet<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Result<TurnContext, ()> {
        let original_user_prompt = runtime.conversation.last_user_content().filter(|c| {
            !c.starts_with("=== tool_result:")
                && !c.starts_with("=== tool_error:")
                && !c.starts_with("[runtime:correction]")
        });
        let retrieval_intent = original_user_prompt
            .map(classify_retrieval_intent)
            .unwrap_or(RetrievalIntent::None);
        let requested_read_path: Option<String> = match &retrieval_intent {
            RetrievalIntent::DirectRead { path, .. } => Some(path.clone()),
            _ => None,
        };
        let direct_read_mode = match &retrieval_intent {
            RetrievalIntent::DirectRead { mode, .. } => Some(*mode),
            _ => None,
        };
        let investigation_required = original_user_prompt
            .map(|prompt| {
                requested_read_path.is_none()
                    && !user_requested_mutation(prompt)
                    && prompt_requires_investigation(prompt)
            })
            .unwrap_or(false);
        let mutation_allowed = original_user_prompt
            .map(|p| user_requested_mutation(p) || user_requested_execution(p))
            .unwrap_or(false);
        let simple_edit_request = original_user_prompt.and_then(requested_simple_edit);
        let tool_surface = original_user_prompt
            .map(|p| {
                select_tool_surface(
                    p,
                    investigation_required,
                    mutation_allowed,
                    requested_read_path.is_some() || !reads_this_turn.is_empty(),
                )
            })
            .unwrap_or(if reads_this_turn.is_empty() {
                ToolSurface::AnswerOnly
            } else {
                ToolSurface::RetrievalFirst
            });
        let investigation_mode = original_user_prompt
            .map(detect_investigation_mode)
            .unwrap_or(InvestigationMode::General);
        let explicit_investigation_path_scope: Option<String> = if investigation_required {
            original_user_prompt.and_then(extract_investigation_path_scope)
        } else {
            None
        };
        let same_scope_reference = investigation_required
            && explicit_investigation_path_scope.is_none()
            && original_user_prompt.is_some_and(has_same_scope_reference);
        let investigation_path_scope: Option<String> =
            if let Some(scope) = explicit_investigation_path_scope {
                Some(scope)
            } else if same_scope_reference {
                trace_runtime_decision(
                    on_event,
                    "anchor_prompt_matched",
                    &[("kind", "same_scope".into())],
                );
                match runtime.anchors.last_scoped_search_scope().map(str::to_string) {
                    Some(scope) => {
                        trace_runtime_decision(
                            on_event,
                            "anchor_resolved",
                            &[("kind", "same_scope".into()), ("scope", scope.clone())],
                        );
                        Some(scope)
                    }
                    None => {
                        trace_runtime_decision(
                            on_event,
                            "anchor_missing",
                            &[("kind", "same_scope".into())],
                        );
                        runtime.finish_with_runtime_answer(
                            NO_LAST_SCOPED_SEARCH_AVAILABLE,
                            AnswerSource::RuntimeTerminal {
                                reason: RuntimeTerminalReason::InsufficientEvidence,
                                rounds: tool_rounds,
                            },
                            on_event,
                        );
                        return Err(());
                    }
                }
            } else {
                None
            };
        trace_runtime_decision(
            on_event,
            "investigation_mode_detected",
            &[
                ("mode", investigation_mode.as_str().into()),
                ("required", investigation_required.to_string()),
            ],
        );
        trace_runtime_decision(
            on_event,
            "investigation_path_scope",
            &[(
                "scope",
                investigation_path_scope
                    .as_deref()
                    .unwrap_or("none")
                    .to_string(),
            )],
        );
        trace_runtime_decision(
            on_event,
            "tool_surface_selected",
            &[("surface", tool_surface.as_str().into())],
        );
        let shell_request = original_user_prompt.and_then(requested_shell_command);
        if !investigation_required && tool_surface != ToolSurface::GitReadOnly {
            if let Some(cmd) = shell_request.as_ref() {
                if !is_permitted_shell_command(cmd) {
                    let first = cmd.split_whitespace().next().unwrap_or(cmd);
                    on_event(RuntimeEvent::Failed {
                        message: format!(
                            "shell command '{}' is not permitted. Allowed: cargo",
                            first
                        ),
                    });
                    return Err(());
                }
            }
        }
        Ok(TurnContext {
            original_user_prompt: original_user_prompt.map(str::to_string),
            retrieval_intent,
            requested_read_path,
            direct_read_mode,
            investigation_required,
            mutation_allowed,
            simple_edit_request,
            tool_surface,
            investigation_mode,
            investigation_path_scope,
            shell_request,
        })
    }
}

fn seed_pending_runtime_call(ctx: &TurnContext, state: &mut TurnState) {
    state.investigation.configure_usage_evidence_policy(usage_lookup_is_broad(
        ctx.investigation_mode,
        ctx.requested_read_path.as_deref(),
        ctx.investigation_path_scope.as_deref(),
    ));
    if !ctx.investigation_required && ctx.tool_surface != ToolSurface::GitReadOnly {
        if let Some(cmd) = ctx.shell_request.as_ref() {
            state.pending_runtime_call = Some(PendingRuntimeCall {
                input: ToolInput::Shell { command: cmd.clone() },
                seeded_pre_generation: true,
            });
        } else if let Some(edit) = ctx.simple_edit_request.as_ref() {
            state.pending_runtime_call = Some(PendingRuntimeCall {
                input: ToolInput::EditFile {
                    path: edit.path.clone(),
                    search: edit.search.clone(),
                    replace: edit.replace.clone(),
                },
                seeded_pre_generation: true,
            });
        } else {
            match &ctx.retrieval_intent {
                RetrievalIntent::DirectRead { path, .. } => {
                    state.pending_runtime_call = Some(PendingRuntimeCall {
                        input: ToolInput::ReadFile { path: path.clone() },
                        seeded_pre_generation: true,
                    });
                }
                RetrievalIntent::DirectoryListing { path } => {
                    state.pending_runtime_call = Some(PendingRuntimeCall {
                        input: ToolInput::ListDir { path: path.clone() },
                        seeded_pre_generation: true,
                    });
                }
                RetrievalIntent::None => {}
            }
        }
    }
}

/// Extracts the absolute file path from an edit_file or write_file pending payload.
/// Both tools use a null-byte-separated format:
///   v2: "v2\x00<abs_path>\x00..."
///   legacy: "<abs_path>\x00..."
fn extract_absolute_path_from_payload(payload: &str) -> Option<String> {
    const SEP: char = '\x00';
    let mut parts = payload.splitn(3, SEP);
    let first = parts.next()?;
    if first == "v2" {
        let abs = parts.next()?;
        if !abs.is_empty() {
            return Some(abs.to_string());
        }
        return None;
    }
    // Legacy: first segment is the absolute path.
    if std::path::Path::new(first).is_absolute() {
        return Some(first.to_string());
    }
    None
}

/// Returns true when the most recent user message in the conversation is an edit_file
/// tool error injected by the runtime. Used to detect the edit-repair failure pattern:
/// model emits garbled edit syntax after a failed edit, producing zero parsed tool calls.
fn last_injected_was_edit_error(conversation: &Conversation) -> bool {
    conversation
        .last_user_content()
        .map(|c| c.starts_with("=== tool_error: edit_file ==="))
        .unwrap_or(false)
}

