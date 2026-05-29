use std::collections::HashSet;

use crate::tools::{ExecutionKind, ToolError, ToolInput, ToolRunResult};

use super::super::super::investigation::investigation::{InvestigationMode, InvestigationState};
use super::super::super::investigation::tool_surface::ToolSurface;
use super::super::super::protocol::response_text::{
    direct_read_fallback_answer, LAST_SEARCH_REPLAYED, LAST_SEARCH_REPLAY_FAILED,
};
use super::super::super::protocol::tool_codec;
use super::super::super::resolve;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{Activity, AnswerSource, RuntimeEvent, RuntimeTerminalReason};
use super::super::tool_round::{run_tool_round, SearchBudget, ToolRoundOutcome};
use super::Runtime;

impl Runtime {
    pub(super) fn run_last_read_file_anchor(
        &mut self,
        path: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let mut last_call_key: Option<String> = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut reads_this_turn: HashSet<String> = HashSet::new();
        let mut requested_read_completed = false;
        let mut disallowed_tool_attempts = 0usize;
        let mut weak_search_query_attempts = 0usize;

        on_event(RuntimeEvent::ActivityChanged(Activity::ExecutingTools {
            tool: "read".to_string(),
            detail: Some(path.clone()),
        }));
        match run_tool_round(
            &self.project_root,
            &self.registry,
            vec![ToolInput::ReadFile { path }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut self.lsp,
            &mut reads_this_turn,
            &mut self.anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed_tool_attempts,
            &mut weak_search_query_attempts,
            false,
            false,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            self.symbol_store.as_ref(),
            on_event,
        ) {
            ToolRoundOutcome::Completed { results, .. } => {
                let answer = direct_read_fallback_answer(&results);
                self.commit_tool_results(results);
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    &answer,
                    AnswerSource::ToolAssisted { rounds: 1 },
                    on_event,
                );
            }
            ToolRoundOutcome::TerminalAnswer {
                results,
                answer,
                reason,
            } => {
                self.commit_tool_results(results);
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    &answer,
                    AnswerSource::RuntimeTerminal { reason, rounds: 1 },
                    on_event,
                );
            }
            ToolRoundOutcome::ApprovalRequired {
                accumulated,
                pending,
            } => {
                if !accumulated.is_empty() {
                    self.commit_tool_results(accumulated);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                }
                self.pending_action = Some(pending.clone());
                on_event(RuntimeEvent::ApprovalRequired {
                    pending,
                    evidence: vec![],
                });
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
            }
            ToolRoundOutcome::RuntimeDispatch { .. } => {
                debug_assert!(
                    false,
                    "RuntimeDispatch is not expected during last-read anchor replay"
                );
                on_event(RuntimeEvent::Failed {
                    message: "Unexpected runtime dispatch during last-read replay.".to_string(),
                });
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
            }
        }
    }

    pub(super) fn run_last_search_anchor(
        &mut self,
        query: String,
        scope: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let input = ToolInput::SearchCode {
            query: query.clone(),
            path: scope.clone(),
        };
        let name = input.tool_name().to_string();

        on_event(RuntimeEvent::ActivityChanged(Activity::ExecutingTools {
            tool: "search".to_string(),
            detail: Some(query.clone()),
        }));
        on_event(RuntimeEvent::ToolCallStarted { name: name.clone() });

        let resolved = match resolve(&self.project_root, &input) {
            Ok(resolved) => resolved,
            Err(error) => {
                let tool_error: ToolError = error.into();
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                self.conversation.push_user(tool_codec::format_tool_error(
                    &name,
                    &tool_error.to_string(),
                ));
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    LAST_SEARCH_REPLAY_FAILED,
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: 1,
                    },
                    on_event,
                );
                return;
            }
        };

        match self.registry.dispatch(resolved) {
            Ok(ToolRunResult::Immediate(output)) => {
                debug_assert!(
                    self.registry
                        .spec_for(&name)
                        .map(|s| s.execution_kind == ExecutionKind::Immediate)
                        .unwrap_or(true),
                    "tool '{name}' returned Immediate but spec declares RequiresApproval"
                );
                if let Some((query, scope)) =
                    self.anchors
                        .record_successful_search(&output, query.clone(), scope.clone())
                {
                    trace_runtime_decision(
                        on_event,
                        "anchor_updated",
                        &[
                            ("kind", "last_search".into()),
                            ("query", query),
                            ("scope", scope.unwrap_or_else(|| "none".into())),
                        ],
                    );
                }
                let summary = tool_codec::render_compact_summary(&output);
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: Some(summary),
                });
                self.commit_tool_results(tool_codec::format_tool_result(&name, &output));
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    LAST_SEARCH_REPLAYED,
                    AnswerSource::ToolAssisted { rounds: 1 },
                    on_event,
                );
            }
            Ok(ToolRunResult::Approval(pending)) => {
                debug_assert!(
                    self.registry
                        .spec_for(&name)
                        .map(|s| s.execution_kind == ExecutionKind::RequiresApproval)
                        .unwrap_or(false),
                    "tool '{name}' requested approval but spec declares Immediate"
                );
                self.pending_action = Some(pending.clone());
                on_event(RuntimeEvent::ApprovalRequired {
                    pending,
                    evidence: vec![],
                });
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
            }
            Err(e) => {
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                self.conversation
                    .push_user(tool_codec::format_tool_error(&name, &e.to_string()));
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    LAST_SEARCH_REPLAY_FAILED,
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: 1,
                    },
                    on_event,
                );
            }
        }
    }
}
