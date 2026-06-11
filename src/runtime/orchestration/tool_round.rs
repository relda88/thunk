use std::collections::HashSet;
use std::path::Path;

use crate::core::config::{InvestigationDepth, RetrievalConfig};
use crate::runtime::index::EmbeddingProvider;
use crate::storage::index::SymbolStore;
use crate::tools::types::LspDefinitionOutput;
use crate::tools::{
    ExecutionKind, PendingAction, ToolError, ToolInput, ToolOutput, ToolRegistry, ToolRunResult,
};

use super::super::investigation::anchors::AnchorState;
use super::super::investigation::classify::is_declaration_line;
use super::super::investigation::investigation::{
    InvestigationMode, InvestigationState, ReadClassification,
};
use super::super::investigation::search_query::{simplify_search_input, weak_search_query_reason};
use super::super::investigation::tool_surface::{
    is_git_read_only_tool_input, tool_allowed_for_surface, ToolSurface,
};
use super::super::lsp::LspManager;
use super::super::paths::{normalize_evidence_path, path_is_within_scope, path_matches_requested};
use super::super::protocol::response_text::*;
use super::super::protocol::tool_codec;
use super::super::trace::trace_runtime_decision;
use super::super::types::{RuntimeEvent, RuntimeTerminalReason};
use super::super::{resolve, ProjectRoot};

/// Maximum number of successful read_file calls allowed in a single turn.
/// Each read injects up to MAX_LINES lines into the prompt; this cap bounds worst-case
/// context growth when the model reads speculatively or drifts into repeated reads.
/// 3 is conservative: a correct investigation needs 1 (search → read → answer);
/// 2-3 accommodates a reasonable follow-up read without runaway context expansion.
pub(crate) const MAX_READS_PER_TURN: usize = 3;

/// Maximum number of distinct search-candidate files that may be read in a single
/// investigation turn.  After two candidate reads, if evidence is still not ready,
/// the runtime terminates cleanly rather than allowing another correction cycle.
pub(crate) const MAX_CANDIDATE_READS_PER_INVESTIGATION: usize = 2;

/// Tracks search_code usage within a single turn.
/// Rules: 1 search always permitted; a second search is permitted only when the first
/// returned zero matches; any further searches are blocked.
pub(crate) struct SearchBudget {
    pub(super) calls: usize,
    last_was_empty: bool,
}

impl SearchBudget {
    pub(crate) fn new() -> Self {
        Self {
            calls: 0,
            last_was_empty: false,
        }
    }

    fn is_allowed(&self) -> bool {
        self.calls == 0 || (self.calls == 1 && self.last_was_empty)
    }

    fn record(&mut self, was_empty: bool) {
        self.calls += 1;
        self.last_was_empty = was_empty;
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.calls >= 2 || (self.calls == 1 && !self.last_was_empty)
    }

    pub(crate) fn empty_retry_exhausted(&self) -> bool {
        self.calls >= 2 && self.last_was_empty
    }

    pub(crate) fn closed_message(&self) -> &'static str {
        if self.calls >= 2 && self.last_was_empty {
            SEARCH_CLOSED_AFTER_EMPTY_RETRY
        } else {
            SEARCH_CLOSED_AFTER_RESULTS
        }
    }
}

/// Returns a stable fingerprint for a tool call, used for consecutive-cycle detection.
/// Null bytes separate fields; they cannot appear in paths, queries, or file content
/// on any supported platform, so false matches are impossible.
fn call_fingerprint(input: &ToolInput) -> String {
    match input {
        ToolInput::ReadFile { path } => format!("read_file\x00{path}"),
        ToolInput::ListDir { path } => format!("list_dir\x00{path}"),
        ToolInput::SearchCode { query, path } => {
            format!(
                "search_code\x00{query}\x00{}",
                path.as_deref().unwrap_or("")
            )
        }
        ToolInput::GitStatus => "git_status".to_string(),
        ToolInput::GitDiff => "git_diff".to_string(),
        ToolInput::GitLog => "git_log".to_string(),
        ToolInput::GitBranch => "git_branch".to_string(),
        ToolInput::GitBranchCreate { name, .. } => format!("git_branch_create\x00{name}"),
        ToolInput::GitBranchSwitch { name } => format!("git_branch_switch\x00{name}"),
        ToolInput::GitCommit { message } => format!("git_commit\x00{message}"),
        ToolInput::GitDiffStaged => "git_diff_staged".to_string(),
        ToolInput::EditFile {
            path,
            search,
            replace,
        } => {
            format!("edit_file\x00{path}\x00{search}\x00{replace}")
        }
        ToolInput::WriteFile { path, content } => {
            format!("write_file\x00{path}\x00{content}")
        }
        ToolInput::Shell { command } => format!("shell\x00{command}"),
        ToolInput::LspDefinition { path, line, col } => {
            format!("lsp_definition\x00{path}\x00{line}\x00{col}")
        }
        ToolInput::WebFetch { url } => format!("web_fetch\x00{url}"),
    }
}

fn is_mutating_tool(input: &ToolInput) -> bool {
    matches!(
        input,
        ToolInput::EditFile { .. }
            | ToolInput::WriteFile { .. }
            | ToolInput::Shell { .. }
            | ToolInput::GitBranchCreate { .. }
            | ToolInput::GitBranchSwitch { .. }
            | ToolInput::GitCommit { .. }
    )
}

fn is_general_doc_like_candidate_path(path: &str) -> bool {
    let normalized = normalize_evidence_path(path);
    let lower = normalized.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    file_name == "readme"
        || file_name.starts_with("readme.")
        || lower
            .split('/')
            .any(|segment| matches!(segment, "doc" | "docs" | "benchmark" | "benchmarks"))
}

/// Outcome of dispatching one round of tool calls.
pub(crate) enum ToolRoundOutcome {
    /// All tools in this round completed immediately; results are ready to push.
    Completed {
        results: String,
        git_acquisition_answer: Option<String>,
    },
    /// The runtime has enough information to end the turn without asking the model
    /// for another synthesis pass.
    TerminalAnswer {
        results: String,
        answer: String,
        reason: RuntimeTerminalReason,
    },
    /// A tool requested approval. Results accumulated before it are preserved.
    /// The turn is now suspended; the caller must store pending and fire the event.
    ApprovalRequired {
        accumulated: String,
        pending: PendingAction,
    },
    /// Two or more consecutive mutation tools requested approval in a single turn.
    /// The caller presents all as a single grouped approval and executes atomically.
    TransactionRequired {
        accumulated: String,
        actions: Vec<PendingAction>,
    },

    /// Runtime has selected the next tool call itself.
    /// The caller must re-enter the normal tool execution loop with this call;
    /// it must not dispatch the tool inline.
    RuntimeDispatch {
        accumulated: String,
        call: ToolInput,
    },
}

/// Dispatches one round of tool calls, accumulating results.
/// Stops at the first tool that requires approval and returns any results
/// accumulated before it alongside the PendingAction.
/// ToolCallStarted is fired for each tool, but ToolCallFinished is NOT fired
/// for the approval-requiring tool — handle_approve/reject fires it after resolution.
///
/// `last_call_key` carries the fingerprint of the most recently executed call across
/// rounds. If the current call matches it, a cycle error is injected instead of
/// dispatching. The key is updated after every non-cycle, non-approval dispatch.
#[allow(clippy::too_many_arguments)] // orchestration function wiring tool dispatch, investigation state, and policy
pub(crate) fn run_tool_round(
    project_root: &ProjectRoot,
    registry: &ToolRegistry,
    calls: Vec<ToolInput>,
    last_call_key: &mut Option<String>,
    search_budget: &mut SearchBudget,
    investigation: &mut InvestigationState,
    lsp: &mut LspManager,
    reads_this_turn: &mut HashSet<String>,
    anchors: &mut AnchorState,
    tool_surface: ToolSurface,
    disallowed_tool_attempts: &mut usize,
    weak_search_query_attempts: &mut usize,
    mutation_allowed: bool,
    investigation_required: bool,
    investigation_mode: InvestigationMode,
    requested_read_path: Option<&str>,
    requested_read_completed: &mut bool,
    investigation_path_scope: Option<&str>,
    symbol_store: Option<&SymbolStore>,
    embedding_provider: Option<&(dyn EmbeddingProvider + Send)>,
    retrieval_config: &RetrievalConfig,
    on_event: &mut dyn FnMut(RuntimeEvent),
) -> ToolRoundOutcome {
    let mut accumulated = String::new();
    let mut git_answer_sections = Vec::new();

    let mut calls_iter = calls.into_iter();
    while let Some(mut input) = calls_iter.next() {
        simplify_search_input(&mut input);
        // Enforce the prompt-derived path scope as an upper bound on search dispatch.
        // None → inject scope (9.1.2 behavior).
        // Some(p) within scope → keep; model narrowed correctly.
        // Some(p) broader than or orthogonal to scope → clamp silently to scope.
        if let (Some(scope), ToolInput::SearchCode { path, .. }) =
            (investigation_path_scope, &mut input)
        {
            match path {
                None => {
                    trace_runtime_decision(
                        on_event,
                        "search_scope_applied",
                        &[
                            ("action", "inject".into()),
                            ("original_path", "none".into()),
                            ("scope", scope.to_string()),
                            ("final_path", scope.to_string()),
                        ],
                    );
                    *path = Some(scope.to_string());
                }
                Some(ref p) if !path_is_within_scope(p, scope) => {
                    trace_runtime_decision(
                        on_event,
                        "search_scope_applied",
                        &[
                            ("action", "clamp".into()),
                            ("original_path", p.to_string()),
                            ("scope", scope.to_string()),
                            ("final_path", scope.to_string()),
                        ],
                    );
                    *path = Some(scope.to_string());
                }
                _ => {}
            }
        }
        let effective_search_input = match &input {
            ToolInput::SearchCode { query, path } => Some((query.clone(), path.clone())),
            _ => None,
        };
        let read_path = match &input {
            ToolInput::ReadFile { path } => Some(path.clone()),
            _ => None,
        };
        // Pre-intercept: if a non-candidate read_file can be deterministically dispatched
        // to the preferred candidate, intercept now — before emitting ToolCallStarted —
        // so no invalid tool events ever appear in the stream.
        if investigation_required
            && investigation.search_produced_results()
            && requested_read_path.is_none()
        {
            if let Some(rp) = read_path.as_deref() {
                if !investigation.is_search_candidate_path(rp)
                    && investigation.non_candidate_read_attempts() == 0
                {
                    let best = investigation
                        .best_candidate_for_mode(investigation_mode)
                        .map(|s| s.to_string());
                    let dispatch_possible = best.as_ref().map_or(false, |c| {
                        let normalized = normalize_evidence_path(c);
                        investigation.is_search_candidate_path(c)
                            && !reads_this_turn.contains(&normalized)
                            && reads_this_turn.len() < MAX_READS_PER_TURN
                            && investigation.candidate_reads_count()
                                < MAX_CANDIDATE_READS_PER_INVESTIGATION
                    });
                    if dispatch_possible {
                        investigation.increment_non_candidate_read_attempts();
                        trace_runtime_decision(
                            on_event,
                            "non_candidate_read_rejected",
                            &[
                                ("path", normalize_evidence_path(rp)),
                                ("mode", investigation_mode.as_str().to_string()),
                                (
                                    "candidate_count",
                                    investigation.search_candidate_count().to_string(),
                                ),
                                (
                                    "preferred_candidate",
                                    best.as_deref().unwrap_or("none").to_string(),
                                ),
                                ("recovery_action", "dispatch".to_string()),
                                ("search_closed", search_budget.is_closed().to_string()),
                            ],
                        );
                        let c = best.unwrap();
                        trace_runtime_decision(
                            on_event,
                            "candidate_selected",
                            &[
                                ("path", normalize_evidence_path(&c)),
                                ("mode", investigation_mode.as_str().to_string()),
                                ("selection_reason", "non_candidate_redirect".to_string()),
                                ("dispatch_possible", "true".to_string()),
                            ],
                        );
                        return ToolRoundOutcome::RuntimeDispatch {
                            accumulated,
                            call: ToolInput::ReadFile { path: c },
                        };
                    }
                }
            }
        }

        let name = input.tool_name().to_string();
        let key = call_fingerprint(&input);
        let is_git_read_only_tool = is_git_read_only_tool_input(&input);
        on_event(RuntimeEvent::ToolCallStarted { name: name.clone() });

        if !tool_allowed_for_surface(&input, tool_surface) {
            *disallowed_tool_attempts += 1;
            trace_runtime_decision(
                on_event,
                "tool_disallowed",
                &[
                    ("tool", name.clone()),
                    ("surface", tool_surface.as_str().into()),
                    ("attempts", disallowed_tool_attempts.to_string()),
                ],
            );
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: None,
            });
            if *disallowed_tool_attempts == 1 {
                accumulated.push_str(&tool_codec::format_tool_error(
                    &name,
                    surface_policy_correction(tool_surface),
                ));
                continue;
            }
            accumulated.push_str(&tool_codec::format_tool_error(
                &name,
                repeated_disallowed_tool_error(tool_surface),
            ));
            return ToolRoundOutcome::TerminalAnswer {
                results: accumulated,
                answer: repeated_disallowed_tool_final_answer().to_string(),
                reason: RuntimeTerminalReason::RepeatedDisallowedTool,
            };
        }

        if tool_surface == ToolSurface::RetrievalFirst && investigation_required {
            if let ToolInput::SearchCode { query, .. } = &input {
                if let Some(reason) = weak_search_query_reason(query) {
                    *weak_search_query_attempts += 1;
                    trace_runtime_decision(
                        on_event,
                        "weak_search_query_rejected",
                        &[
                            ("query", query.clone()),
                            ("reason", reason.into()),
                            ("attempts", weak_search_query_attempts.to_string()),
                        ],
                    );
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: name.clone(),
                        summary: None,
                    });
                    if *weak_search_query_attempts == 1 {
                        let correction = weak_search_query_correction(reason);
                        accumulated.push_str(&tool_codec::format_tool_error(&name, &correction));
                        continue;
                    }
                    accumulated.push_str(&tool_codec::format_tool_error(
                        &name,
                        "repeated weak search query for this investigation turn.",
                    ));
                    return ToolRoundOutcome::TerminalAnswer {
                        results: accumulated,
                        answer: repeated_weak_search_query_final_answer().to_string(),
                        reason: RuntimeTerminalReason::RepeatedWeakSearchQuery,
                    };
                }
            }
        }

        if is_mutating_tool(&input) && !mutation_allowed {
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: None,
            });
            accumulated.push_str(&tool_codec::format_tool_error(
                &name,
                READ_ONLY_TOOL_POLICY_ERROR,
            ));
            continue;
        }

        if matches!(input, ToolInput::ListDir { .. })
            && investigation_required
            && !investigation.search_attempted()
        {
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: None,
            });
            accumulated.push_str(&tool_codec::format_tool_error(
                &name,
                LIST_DIR_BEFORE_SEARCH_BLOCKED,
            ));
            continue;
        }

        if let (Some(requested), ToolInput::ReadFile { path }) = (requested_read_path, &input) {
            if !path_matches_requested(path, requested) {
                let error = format!(
                    "read_file path `{path}` does not match the requested path `{requested}`"
                );
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                accumulated.push_str(&tool_codec::format_tool_error(&name, &error));
                return ToolRoundOutcome::TerminalAnswer {
                    results: accumulated,
                    answer: read_path_mismatch_final_answer(requested, path),
                    reason: RuntimeTerminalReason::ReadFileFailed,
                };
            }
        }

        // Per-turn search budget: 1 search always allowed; a second only when the first
        // returned no results; further searches are always blocked.
        if matches!(input, ToolInput::SearchCode { .. })
            && !search_budget.is_allowed()
            && !(investigation.definition_refinement_issued() && search_budget.calls == 1)
        {
            if search_budget.empty_retry_exhausted()
                && !investigation.search_produced_results()
                && investigation.files_read_count() == 0
            {
                trace_runtime_decision(
                    on_event,
                    "terminal_insufficient_evidence",
                    &[
                        ("reason", "empty_search_retry_exhausted".into()),
                        ("search_calls", search_budget.calls.to_string()),
                        ("files_read", investigation.files_read_count().to_string()),
                    ],
                );
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                return ToolRoundOutcome::TerminalAnswer {
                    results: accumulated,
                    answer: insufficient_evidence_final_answer().to_string(),
                    reason: RuntimeTerminalReason::InsufficientEvidence,
                };
            }
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: None,
            });
            accumulated.push_str(&tool_codec::format_tool_error(
                &name,
                SEARCH_BUDGET_EXCEEDED,
            ));
            continue;
        }

        // Dedup: block re-reads of the same file within the same turn.
        // The file's contents are already in context; re-reading only inflates the prompt.
        if let Some(rp) = read_path.as_deref() {
            let normalized = normalize_evidence_path(rp);
            if reads_this_turn.contains(&normalized) {
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                accumulated.push_str(&tool_codec::format_tool_error(
                    &name,
                    DUPLICATE_READ_REJECTED,
                ));
                continue;
            }
        }

        // Non-candidate read guard: after search results are known, block read_file calls
        // that target files outside the candidate set.  Skipped before any search has
        // produced results (guard condition: search_produced_results()) and on direct-read
        // turns (requested_read_path.is_some()).  Mutation and git flows are unaffected
        // because investigation_required is false on those turns.
        // First offense: correction injected, model may retry with a matched file.
        // Repeated offense within the same round: terminal.
        if investigation_required
            && investigation.search_produced_results()
            && requested_read_path.is_none()
        {
            if let Some(rp) = read_path.as_deref() {
                if matches!(investigation_mode, InvestigationMode::General)
                    && investigation.candidate_reads_count() == 0
                    && investigation.is_search_candidate_path(rp)
                {
                    let best = investigation
                        .best_candidate_for_mode(InvestigationMode::General)
                        .map(|s| s.to_string());
                    if let Some(candidate) = best {
                        if is_general_doc_like_candidate_path(rp)
                            && normalize_evidence_path(&candidate) != normalize_evidence_path(rp)
                        {
                            trace_runtime_decision(
                                on_event,
                                "general_doc_candidate_redirected",
                                &[
                                    ("rejected_path", normalize_evidence_path(rp)),
                                    ("candidate_path", normalize_evidence_path(&candidate)),
                                ],
                            );
                            on_event(RuntimeEvent::ToolCallFinished {
                                name: name.clone(),
                                summary: None,
                            });
                            return ToolRoundOutcome::RuntimeDispatch {
                                accumulated,
                                call: ToolInput::ReadFile { path: candidate },
                            };
                        }
                    }
                }
                if !investigation.is_search_candidate_path(rp) {
                    let attempts = investigation.increment_non_candidate_read_attempts();
                    let best = investigation
                        .best_candidate_for_mode(investigation_mode)
                        .map(|s| s.to_string());
                    // Dispatch is possible when: first offense, candidate is in the valid
                    // candidate set, not already read this turn, and neither the per-turn
                    // read cap nor the per-investigation candidate-read cap is exhausted.
                    let dispatch_possible = attempts == 1
                        && best.as_ref().map_or(false, |c| {
                            let normalized = normalize_evidence_path(c);
                            investigation.is_search_candidate_path(c)
                                && !reads_this_turn.contains(&normalized)
                                && reads_this_turn.len() < MAX_READS_PER_TURN
                                && investigation.candidate_reads_count()
                                    < MAX_CANDIDATE_READS_PER_INVESTIGATION
                        });
                    trace_runtime_decision(
                        on_event,
                        "non_candidate_read_rejected",
                        &[
                            ("path", normalize_evidence_path(rp)),
                            ("mode", investigation_mode.as_str().to_string()),
                            (
                                "candidate_count",
                                investigation.search_candidate_count().to_string(),
                            ),
                            (
                                "preferred_candidate",
                                best.as_deref().unwrap_or("none").to_string(),
                            ),
                            (
                                "recovery_action",
                                if dispatch_possible {
                                    "dispatch"
                                } else if attempts == 1 {
                                    "correction"
                                } else {
                                    "terminal"
                                }
                                .to_string(),
                            ),
                            ("search_closed", search_budget.is_closed().to_string()),
                        ],
                    );
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: name.clone(),
                        summary: None,
                    });
                    if attempts == 1 {
                        if let Some(ref c) = best {
                            trace_runtime_decision(
                                on_event,
                                "candidate_selected",
                                &[
                                    ("path", normalize_evidence_path(c)),
                                    ("mode", investigation_mode.as_str().to_string()),
                                    (
                                        "selection_reason",
                                        if dispatch_possible {
                                            "non_candidate_redirect"
                                        } else {
                                            "correction_hint"
                                        }
                                        .to_string(),
                                    ),
                                    ("dispatch_possible", dispatch_possible.to_string()),
                                ],
                            );
                            if dispatch_possible {
                                return ToolRoundOutcome::RuntimeDispatch {
                                    accumulated,
                                    call: ToolInput::ReadFile { path: c.clone() },
                                };
                            }
                        }
                        accumulated.push_str(&tool_codec::format_tool_error(
                            &name,
                            &non_candidate_read_correction(rp, best.as_deref()),
                        ));
                        continue;
                    }
                    accumulated.push_str(&tool_codec::format_tool_error(
                        &name,
                        &format!(
                            "`{rp}` is not in the search results — repeated non-candidate read."
                        ),
                    ));
                    return ToolRoundOutcome::TerminalAnswer {
                        results: accumulated,
                        answer: non_candidate_read_terminal_answer().to_string(),
                        reason: RuntimeTerminalReason::ReadFileFailed,
                    };
                }
            }
        }

        // Candidate-read cap: once two matched candidates have been read without
        // useful evidence, do not allow the model to keep reading current candidates.
        // Bypassed during the deepening phase so hop reads are not blocked.
        if investigation_required
            && !investigation.evidence_ready()
            && investigation.candidate_reads_count() >= MAX_CANDIDATE_READS_PER_INVESTIGATION
            && !investigation.deepening_phase_active
        {
            if let Some(rp) = read_path.as_deref() {
                if investigation.is_search_candidate_path(rp) {
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", normalize_evidence_path(rp)),
                            ("accepted", "false".into()),
                            ("reason", "candidate_read_limit_exhausted".into()),
                            (
                                "candidate_reads",
                                investigation.candidate_reads_count().to_string(),
                            ),
                        ],
                    );
                    trace_runtime_decision(
                        on_event,
                        "terminal_insufficient_evidence",
                        &[
                            ("reason", "candidate_read_limit_exhausted".into()),
                            (
                                "candidate_reads",
                                investigation.candidate_reads_count().to_string(),
                            ),
                        ],
                    );
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: name.clone(),
                        summary: None,
                    });
                    accumulated.push_str(&tool_codec::format_tool_error(
                        &name,
                        CANDIDATE_READ_CAP_EXCEEDED,
                    ));
                    return ToolRoundOutcome::TerminalAnswer {
                        results: accumulated,
                        answer: ungrounded_investigation_final_answer().to_string(),
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                    };
                }
            }
        }

        // Per-turn read cap: block new reads once MAX_READS_PER_TURN unique files have been read.
        // reads_this_turn.len() counts only successful reads, so the cap is exact.
        if read_path.is_some() && reads_this_turn.len() >= MAX_READS_PER_TURN {
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: None,
            });
            accumulated.push_str(&tool_codec::format_tool_error(&name, READ_CAP_EXCEEDED));
            continue;
        }

        if last_call_key.as_deref() == Some(key.as_str()) {
            if matches!(input, ToolInput::SearchCode { .. })
                && search_budget.calls > 0
                && search_budget.last_was_empty
                && !investigation.search_produced_results()
                && investigation.files_read_count() == 0
            {
                trace_runtime_decision(
                    on_event,
                    "terminal_insufficient_evidence",
                    &[
                        ("reason", "empty_search_duplicate_retry".into()),
                        ("search_calls", search_budget.calls.to_string()),
                        ("files_read", investigation.files_read_count().to_string()),
                    ],
                );
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                return ToolRoundOutcome::TerminalAnswer {
                    results: accumulated,
                    answer: insufficient_evidence_final_answer().to_string(),
                    reason: RuntimeTerminalReason::InsufficientEvidence,
                };
            }
            let msg = format!("{name} called with identical arguments twice in a row");
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: None,
            });
            accumulated.push_str(&tool_codec::format_tool_error(&name, &msg));
            // Do not update last_call_key: keep the same fingerprint so a third
            // consecutive identical call is also blocked.
            continue;
        }

        let resolved = match resolve(project_root, &input) {
            Ok(resolved) => resolved,
            Err(error) => {
                let tool_error: ToolError = error.into();
                let error = tool_error.to_string();
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                if is_git_read_only_tool {
                    git_answer_sections.push(git_acquisition_answer_section(&name, &error));
                }
                accumulated.push_str(&tool_codec::format_tool_error(&name, &error));
                if let Some(path) = read_path {
                    return ToolRoundOutcome::TerminalAnswer {
                        results: accumulated,
                        answer: read_failure_final_answer(&path, &error),
                        reason: RuntimeTerminalReason::ReadFileFailed,
                    };
                }
                if is_mutating_tool(&input) {
                    return ToolRoundOutcome::TerminalAnswer {
                        results: accumulated,
                        answer: mutation_input_rejected_final_answer(&name, &error),
                        reason: RuntimeTerminalReason::MutationFailed,
                    };
                }
                continue;
            }
        };

        // LSP intercept: must run before registry.dispatch() because Tool::run() is &self
        // but LspManager::query_definition() requires &mut self.
        if let super::super::project::ResolvedToolInput::LspDefinition { path, line, col } =
            &resolved
        {
            let path = path.clone();
            let line = *line;
            let col = *col;
            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: name.clone(),
                        summary: None,
                    });
                    accumulated.push_str(&tool_codec::format_tool_error(&name, &e.to_string()));
                    *last_call_key = Some(key);
                    continue;
                }
            };
            let output = match lsp.query_definition(
                Path::new(&path),
                &source,
                line as usize,
                col as usize,
            ) {
                Ok(locations) => {
                    let (target_path, target_line) = locations
                        .first()
                        .map(|l| {
                            let abs = l.path.to_string_lossy().into_owned();
                            let rel = Path::new(&abs)
                                .strip_prefix(project_root.path())
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or(abs);
                            (rel, l.line as u32)
                        })
                        .unwrap_or_default();
                    ToolOutput::LspDefinition(LspDefinitionOutput {
                        source_path: path.clone(),
                        target_path,
                        target_line,
                    })
                }
                Err(_) => ToolOutput::LspDefinition(LspDefinitionOutput {
                    source_path: path.clone(),
                    target_path: String::new(),
                    target_line: 0,
                }),
            };
            if let ToolOutput::LspDefinition(ref d) = output {
                if !d.target_path.is_empty() {
                    investigation
                        .graph
                        .record_definition_target(&d.source_path, &d.target_path);

                    if lsp.is_enabled() {
                        let target_abs = project_root.path().join(&d.target_path);
                        if let Ok(target_source) = std::fs::read_to_string(&target_abs) {
                            if let Ok(Some(hover_text)) = lsp.query_hover(
                                &target_abs,
                                &target_source,
                                d.target_line as usize,
                                1,
                            ) {
                                trace_runtime_decision(
                                    on_event,
                                    "lsp_hover_injected",
                                    &[
                                        ("path", d.target_path.clone()),
                                        ("line", d.target_line.to_string()),
                                    ],
                                );
                                accumulated.push_str(&format!(
                                    "\n=== lsp_hover: {} ===\n{}\n=== /lsp_hover ===\n",
                                    d.target_path, hover_text
                                ));
                            }
                        }
                    }
                }
            }
            let summary = tool_codec::render_compact_summary(&output);
            on_event(RuntimeEvent::ToolCallFinished {
                name: name.clone(),
                summary: Some(summary),
            });
            accumulated.push_str(&tool_codec::format_tool_result(&name, &output));
            *last_call_key = Some(key);
            continue;
        }

        match registry.dispatch(resolved) {
            Ok(ToolRunResult::Immediate(output)) => {
                // Guard: spec must agree that this tool is Immediate.
                // A mismatch means the spec() and run() implementations are out of sync.
                debug_assert!(
                    registry
                        .spec_for(&name)
                        .map(|s| s.execution_kind == ExecutionKind::Immediate)
                        .unwrap_or(true),
                    "tool '{name}' returned Immediate but spec declares RequiresApproval"
                );
                // For agent-turn search_code, apply hybrid vector augmentation if configured.
                let output = if name == "search_code" {
                    let query_str = effective_search_input
                        .as_ref()
                        .map(|(q, _)| q.as_str())
                        .unwrap_or("");
                    let use_vector = retrieval_config.vector_weight > 0.0
                        && embedding_provider.is_some()
                        && symbol_store.is_some();
                    let skip_reason = if retrieval_config.vector_weight <= 0.0 {
                        "vec_weight_zero"
                    } else if embedding_provider.is_none() {
                        "no_provider"
                    } else {
                        "no_store"
                    };
                    trace_runtime_decision(
                        on_event,
                        "hybrid_search_decision",
                        &[
                            (
                                "use_vector",
                                if use_vector { "true" } else { "false" }.into(),
                            ),
                            (
                                "reason",
                                if use_vector { "enabled" } else { skip_reason }.into(),
                            ),
                        ],
                    );
                    if use_vector {
                        let (augmented, was_applied) = try_vector_augment(
                            query_str,
                            output,
                            symbol_store,
                            embedding_provider,
                            retrieval_config,
                            project_root,
                            on_event,
                        );
                        if was_applied {
                            investigation.record_vector_augmented();
                        }
                        augmented
                    } else {
                        output
                    }
                } else {
                    output
                };
                // Record search results against the per-turn budget and investigation state.
                let search_closed_message = if name == "search_code" {
                    if let Some((query, scope)) = effective_search_input.clone() {
                        if let Some((query, scope)) =
                            anchors.record_successful_search(&output, query, scope)
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
                    }
                    let was_empty = investigation.record_search_results(
                        &output,
                        effective_search_input.as_ref().map(|(q, _)| q.as_str()),
                        investigation_mode,
                        on_event,
                    );
                    search_budget.record(was_empty);
                    search_budget
                        .is_closed()
                        .then(|| search_budget.closed_message())
                } else {
                    None
                };
                // Track successful file reads for evidence grounding and dedup.
                let read_recovery = if name == "read_file" {
                    if let Some(path) = anchors.record_successful_read(&output) {
                        trace_runtime_decision(
                            on_event,
                            "anchor_updated",
                            &[("kind", "last_read_file".into()), ("path", path)],
                        );
                    }
                    let classification = if requested_read_path.is_some() {
                        ReadClassification::Direct
                    } else {
                        ReadClassification::Candidate
                    };
                    let recovery = investigation.record_read_result(
                        &output,
                        investigation_mode,
                        classification,
                        on_event,
                    );
                    if let Some(requested) = requested_read_path {
                        if let Some(rp) = read_path.as_deref() {
                            if normalize_evidence_path(rp) == normalize_evidence_path(requested) {
                                *requested_read_completed = true;
                            }
                        }
                    }
                    // Record path so a repeat read in the same turn is blocked.
                    if let Some(rp) = read_path.as_deref() {
                        reads_this_turn.insert(normalize_evidence_path(rp));
                    }
                    recovery
                } else {
                    None
                };
                let summary = tool_codec::render_compact_summary(&output);
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: Some(summary),
                });
                if name == "read_file" {
                    if let ToolOutput::FileContents(ref fc) = output {
                        on_event(RuntimeEvent::FileReadFinished {
                            path: fc.path.clone(),
                            line_count: fc.total_lines,
                            content: fc.contents.clone(),
                        });
                        investigation.graph.record_read(&fc.path, &fc.contents);
                    }
                }
                if is_git_read_only_tool {
                    git_answer_sections.push(git_acquisition_answer_section(
                        &name,
                        &tool_codec::render_output(&output),
                    ));
                }
                let result_formatted = if name == "search_code"
                    && matches!(investigation_mode, InvestigationMode::DefinitionLookup)
                {
                    tool_codec::format_tool_result_definition_ordered(&name, &output)
                } else {
                    tool_codec::format_tool_result(&name, &output)
                };
                accumulated.push_str(&result_formatted);
                if name == "search_code" {
                    if let Some(hint) = investigation.candidate_preference_hint(investigation_mode)
                    {
                        accumulated.push_str(&hint);
                        accumulated.push_str("\n\n");
                    }
                    if let Some(message) = search_closed_message {
                        accumulated.push_str(message);
                        accumulated.push_str("\n\n");
                    }
                    if matches!(investigation_mode, InvestigationMode::UsageLookup) {
                        if let Some(path) = investigation.preferred_usage_candidate() {
                            trace_runtime_decision(
                                on_event,
                                "usage_candidate_selected",
                                &[
                                    ("path", path.to_string()),
                                    ("mode", investigation_mode.as_str().to_string()),
                                    ("selection_reason", "initial_after_search".to_string()),
                                    ("dispatch_possible", "true".to_string()),
                                ],
                            );
                            return ToolRoundOutcome::RuntimeDispatch {
                                accumulated,
                                call: ToolInput::ReadFile {
                                    path: path.to_string(),
                                },
                            };
                        }
                    }
                    if matches!(investigation_mode, InvestigationMode::DefinitionLookup) {
                        if let Some(store) = symbol_store {
                            if let Some((query, _)) = &effective_search_input {
                                let root_str = project_root.path().to_string_lossy().into_owned();
                                match store.lookup_symbol(&root_str, query) {
                                    Ok(records) if !records.is_empty() => {
                                        let paths: Vec<String> = records
                                            .into_iter()
                                            .take(5)
                                            .map(|r| r.file_path)
                                            .collect();
                                        let count = paths.len();
                                        investigation.inject_index_candidates(paths);
                                        trace_runtime_decision(
                                            on_event,
                                            "index_hit",
                                            &[
                                                ("query", query.clone()),
                                                ("candidate_count", count.to_string()),
                                            ],
                                        );
                                    }
                                    Ok(_) | Err(_) => {
                                        trace_runtime_decision(
                                            on_event,
                                            "index_miss",
                                            &[("query", query.clone())],
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if matches!(investigation_mode, InvestigationMode::DefinitionLookup) {
                        if let ToolOutput::SearchResults(ref results) = output {
                            if results.truncated
                                && investigation.first_definition_candidate().is_none()
                                && !investigation.definition_refinement_issued()
                            {
                                if let Some((original_query, scope)) = &effective_search_input {
                                    investigation.set_definition_refinement_issued();
                                    let refined_query = format!("fn {}", original_query);
                                    trace_runtime_decision(
                                        on_event,
                                        "definition_refinement_dispatch",
                                        &[
                                            ("original_query", original_query.to_string()),
                                            ("refined_query", refined_query.clone()),
                                        ],
                                    );
                                    return ToolRoundOutcome::RuntimeDispatch {
                                        accumulated,
                                        call: ToolInput::SearchCode {
                                            query: refined_query,
                                            path: scope.clone(),
                                        },
                                    };
                                }
                            }
                        }
                    }
                    if matches!(investigation_mode, InvestigationMode::DefinitionLookup)
                        && lsp.is_enabled()
                    {
                        if let ToolOutput::SearchResults(ref results) = output {
                            if let Some(def_path) = investigation.first_definition_candidate() {
                                if def_path.ends_with(".rs") {
                                    // Non-Rust files: rust-analyzer cannot serve definitions;
                                    // skip LSP seeding and fall through to candidate read path.
                                    let candidate_matches =
                                        results.matches.iter().filter(|m| m.file == def_path);
                                    let best_match = candidate_matches
                                        .clone()
                                        .find(|m| is_declaration_line(&m.line))
                                        .or_else(|| {
                                            results.matches.iter().find(|m| m.file == def_path)
                                        });
                                    if let Some(m) = best_match {
                                        let col = effective_search_input
                                            .as_ref()
                                            .and_then(|(q, _)| m.line.find(q.as_str()))
                                            .map(|off| off + 1)
                                            .unwrap_or(1);
                                        trace_runtime_decision(
                                            on_event,
                                            "lsp_definition_seeded",
                                            &[
                                                ("path", m.file.clone()),
                                                ("line", m.line_number.to_string()),
                                                ("col", col.to_string()),
                                                ("candidate", def_path.to_string()),
                                            ],
                                        );
                                        return ToolRoundOutcome::RuntimeDispatch {
                                            accumulated,
                                            call: ToolInput::LspDefinition {
                                                path: m.file.clone(),
                                                line: m.line_number as u32,
                                                col: col as u32,
                                            },
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
                let has_read_recovery = read_recovery.is_some();
                if let Some((path, kind)) = read_recovery {
                    trace_runtime_decision(
                        on_event,
                        "recovery_issued",
                        &[("kind", kind.as_str().into()), ("path", path.clone())],
                    );
                    return ToolRoundOutcome::RuntimeDispatch {
                        accumulated,
                        call: ToolInput::ReadFile { path },
                    };
                }
                if name == "read_file"
                    && !has_read_recovery
                    && matches!(investigation_mode, InvestigationMode::UsageLookup)
                    && investigation.investigation_depth != InvestigationDepth::Shallow
                {
                    if let Some(path) = investigation.next_usage_evidence_candidate() {
                        trace_runtime_decision(
                            on_event,
                            "usage_candidate_selected",
                            &[
                                ("path", path.to_string()),
                                ("mode", investigation_mode.as_str().to_string()),
                                ("selection_reason", "additional_usage_evidence".to_string()),
                                ("dispatch_possible", "true".to_string()),
                                (
                                    "useful_candidate_reads",
                                    investigation.useful_candidate_reads_count().to_string(),
                                ),
                            ],
                        );
                        return ToolRoundOutcome::RuntimeDispatch {
                            accumulated,
                            call: ToolInput::ReadFile {
                                path: path.to_string(),
                            },
                        };
                    } else if let Some(def_path) = investigation.first_definition_site_candidate() {
                        let normalized = normalize_evidence_path(&def_path);
                        if !reads_this_turn.contains(&normalized) {
                            trace_runtime_decision(
                                on_event,
                                "usage_candidate_selected",
                                &[
                                    ("path", def_path.to_string()),
                                    ("mode", investigation_mode.as_str().to_string()),
                                    (
                                        "selection_reason",
                                        "definition_after_usage_exhausted".to_string(),
                                    ),
                                    ("dispatch_possible", "true".to_string()),
                                ],
                            );
                            let path = def_path.to_string();
                            investigation.set_definition_site_dispatched(&path);
                            return ToolRoundOutcome::RuntimeDispatch {
                                accumulated,
                                call: ToolInput::ReadFile { path },
                            };
                        }
                    }
                }
                *last_call_key = Some(key);
            }
            Ok(ToolRunResult::Approval(pending)) => {
                // Guard: spec must agree that this tool requires approval.
                debug_assert!(
                    registry
                        .spec_for(&name)
                        .map(|s| s.execution_kind == ExecutionKind::RequiresApproval)
                        .unwrap_or(true),
                    "tool '{name}' returned Approval but spec declares Immediate"
                );
                // Collect any consecutive edit_file/write_file approvals from remaining calls
                // into a transaction. ToolCallStarted fires for each during collection;
                // ToolCallFinished fires during execute_transaction() after approval.
                let mut tx_actions = vec![pending];
                for remaining in calls_iter.by_ref() {
                    if !matches!(
                        remaining,
                        ToolInput::EditFile { .. } | ToolInput::WriteFile { .. }
                    ) {
                        break;
                    }
                    let r_name = remaining.tool_name().to_string();
                    on_event(RuntimeEvent::ToolCallStarted {
                        name: r_name.clone(),
                    });
                    match resolve(project_root, &remaining) {
                        Ok(resolved) => match registry.dispatch(resolved) {
                            Ok(ToolRunResult::Approval(r_pending)) => {
                                tx_actions.push(r_pending);
                            }
                            _ => break,
                        },
                        Err(_) => break,
                    }
                }
                if tx_actions.len() == 1 {
                    return ToolRoundOutcome::ApprovalRequired {
                        accumulated,
                        pending: tx_actions.remove(0),
                    };
                }
                return ToolRoundOutcome::TransactionRequired {
                    accumulated,
                    actions: tx_actions,
                };
            }
            Err(e) => {
                let error = e.to_string();
                on_event(RuntimeEvent::ToolCallFinished {
                    name: name.clone(),
                    summary: None,
                });
                if is_git_read_only_tool {
                    git_answer_sections.push(git_acquisition_answer_section(&name, &error));
                }
                accumulated.push_str(&tool_codec::format_tool_error(&name, &error));
                if let Some(path) = read_path {
                    return ToolRoundOutcome::TerminalAnswer {
                        results: accumulated,
                        answer: read_failure_final_answer(&path, &error),
                        reason: RuntimeTerminalReason::ReadFileFailed,
                    };
                }
                if let ToolInput::EditFile { path, .. } = &input {
                    if error.contains("search text not found") {
                        return ToolRoundOutcome::TerminalAnswer {
                            results: accumulated,
                            answer: seeded_edit_search_not_found_answer(path),
                            reason: RuntimeTerminalReason::MutationFailed,
                        };
                    }
                }
                // Do NOT update last_call_key on error: a failed call should not block
                // an identical retry. Cycle detection applies only to successful executions.
            }
        }
    }

    ToolRoundOutcome::Completed {
        results: accumulated,
        git_acquisition_answer: render_git_acquisition_answer(git_answer_sections),
    }
}

/// Returns `(output, was_applied)` where `was_applied` is true only when vector
/// candidates were merged into the keyword results.
pub(super) fn try_vector_augment(
    query: &str,
    keyword_output: ToolOutput,
    symbol_store: Option<&SymbolStore>,
    embedding_provider: Option<&(dyn EmbeddingProvider + Send)>,
    retrieval_config: &RetrievalConfig,
    project_root: &ProjectRoot,
    on_event: &mut dyn FnMut(RuntimeEvent),
) -> (ToolOutput, bool) {
    let ToolOutput::SearchResults(mut results) = keyword_output else {
        return (keyword_output, false);
    };

    let store = match symbol_store {
        Some(s) => s,
        None => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "no_store".into())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
    };
    let provider = match embedding_provider {
        Some(p) => p,
        None => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "no_provider".into())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
    };

    let project_root_str = project_root.path().to_string_lossy().to_string();

    match store.embedding_count(&project_root_str) {
        Ok(0) => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "no_embeddings".into())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
        Ok(n) if n > 10_000 => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "cap_exceeded".into()), ("n", n.to_string())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
        Err(_) => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "store_error".into())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
        _ => {}
    }

    let query_vec = match provider.embed(&[query.to_string()]) {
        Ok(mut vecs) if !vecs.is_empty() => vecs.remove(0),
        _ => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "embed_failed".into())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
    };

    let vector_files = match store.cosine_search(&project_root_str, &query_vec, 10) {
        Ok(files) => files,
        Err(_) => {
            trace_runtime_decision(
                on_event,
                "vector_augment_skipped",
                &[("reason", "cosine_search_failed".into())],
            );
            return (ToolOutput::SearchResults(results), false);
        }
    };

    if vector_files.is_empty() {
        trace_runtime_decision(
            on_event,
            "vector_augment_skipped",
            &[("reason", "no_vector_results".into())],
        );
        return (ToolOutput::SearchResults(results), false);
    }

    let vector_weight = retrieval_config.vector_weight;
    let keyword_count = results.matches.len();
    let vector_count = vector_files.len();
    results.matches = merge_keyword_vector(results.matches, &vector_files, vector_weight);
    trace_runtime_decision(
        on_event,
        "vector_augment_applied",
        &[
            ("weight", format!("{vector_weight:.2}")),
            ("keyword_count", keyword_count.to_string()),
            ("vector_count", vector_count.to_string()),
        ],
    );
    (ToolOutput::SearchResults(results), true)
}

fn merge_keyword_vector(
    matches: Vec<crate::tools::types::SearchMatch>,
    vector_files: &[String],
    vector_weight: f32,
) -> Vec<crate::tools::types::SearchMatch> {
    if matches.is_empty() || vector_files.is_empty() {
        return matches;
    }

    let mut file_order: Vec<String> = Vec::new();
    let mut by_file: Vec<(String, Vec<crate::tools::types::SearchMatch>)> = Vec::new();

    for m in matches {
        if let Some(idx) = file_order.iter().position(|f| f == &m.file) {
            by_file[idx].1.push(m);
        } else {
            file_order.push(m.file.clone());
            by_file.push((m.file.clone(), vec![m]));
        }
    }

    let n_kw = file_order.len();
    let n_vec = vector_files.len();

    let mut scored: Vec<(f32, usize)> = file_order
        .iter()
        .enumerate()
        .map(|(kw_pos, file)| {
            let kw_score = 1.0 - kw_pos as f32 / n_kw.max(1) as f32;
            let vec_score = vector_files
                .iter()
                .position(|f| f == file)
                .map(|vec_pos| 1.0 - vec_pos as f32 / n_vec.max(1) as f32)
                .unwrap_or(0.0);
            let score = (1.0 - vector_weight) * kw_score + vector_weight * vec_score;
            (score, kw_pos)
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    scored
        .into_iter()
        .flat_map(|(_, idx)| by_file[idx].1.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use tempfile::TempDir;

    use super::*;
    use crate::core::config::LspConfig;
    use crate::runtime::ProjectRoot;
    use crate::tools::types::FileContentsOutput;
    use crate::tools::{
        default_registry, ExecutionKind, Tool, ToolError, ToolOutput, ToolRunResult, ToolSpec,
    };

    struct CountingReadTool {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for CountingReadTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "read_file",
                description: "counting read tool",
                input_hint: "path",
                execution_kind: ExecutionKind::Immediate,
                default_risk: None,
            }
        }

        fn run(
            &self,
            _input: &crate::runtime::ResolvedToolInput,
        ) -> Result<ToolRunResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolRunResult::Immediate(ToolOutput::FileContents(
                FileContentsOutput {
                    path: "counted.txt".into(),
                    contents: "counted".into(),
                    total_lines: 1,
                    truncated: false,
                },
            )))
        }
    }

    fn temp_root() -> (TempDir, ProjectRoot, ToolRegistry) {
        let dir = TempDir::new().unwrap();
        let root = ProjectRoot::new(dir.path().to_path_buf()).unwrap();
        let registry = default_registry().with_project_root(root.as_path_buf());
        (dir, root, registry)
    }

    fn run_round(
        root: &ProjectRoot,
        registry: &ToolRegistry,
        calls: Vec<ToolInput>,
        tool_surface: ToolSurface,
        investigation_required: bool,
    ) -> ToolRoundOutcome {
        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut lsp = LspManager::new(&LspConfig::default(), std::path::Path::new("."));
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed_tool_attempts = 0usize;
        let mut weak_search_query_attempts = 0usize;

        run_tool_round(
            root,
            registry,
            calls,
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut lsp,
            &mut reads_this_turn,
            &mut anchors,
            tool_surface,
            &mut disallowed_tool_attempts,
            &mut weak_search_query_attempts,
            false,
            investigation_required,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        )
    }

    #[test]
    fn resolver_runs_before_dispatch() {
        let dir = TempDir::new().unwrap();
        let root = ProjectRoot::new(dir.path().to_path_buf()).unwrap();
        let outside_file = root.path().parent().unwrap().join(format!(
            "outside-{}.txt",
            dir.path()
                .file_name()
                .expect("temp dir has a file name")
                .to_string_lossy()
        ));
        fs::write(&outside_file, "outside\n").unwrap();

        let mut registry = ToolRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register(CountingReadTool {
            calls: Arc::clone(&calls),
        });

        let outcome = run_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "../outside.txt".into(),
            }],
            ToolSurface::RetrievalFirst,
            false,
        );

        assert!(matches!(outcome, ToolRoundOutcome::TerminalAnswer { .. }));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "resolver failure must prevent tool dispatch"
        );
        fs::remove_file(outside_file).unwrap();
    }

    #[test]
    fn invalid_read_outside_root_becomes_tool_error() {
        let (_dir, root, registry) = temp_root();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("outside.txt");
        fs::write(&outside_file, "outside\n").unwrap();

        let outcome = run_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: outside_file.display().to_string(),
            }],
            ToolSurface::RetrievalFirst,
            false,
        );

        let ToolRoundOutcome::TerminalAnswer { results, .. } = outcome else {
            panic!("read failure should terminate");
        };
        assert!(results.contains("=== tool_error: read_file ==="));
        assert!(results.contains("invalid tool input:"));
        assert!(results.contains("escapes project root"));
    }

    #[test]
    fn invalid_list_scope_outside_root_becomes_tool_error() {
        let (_dir, root, registry) = temp_root();
        let outside = TempDir::new().unwrap();

        let outcome = run_round(
            &root,
            &registry,
            vec![ToolInput::ListDir {
                path: outside.path().display().to_string(),
            }],
            ToolSurface::RetrievalFirst,
            false,
        );

        let ToolRoundOutcome::Completed { results, .. } = outcome else {
            panic!("invalid list_dir scope should stay in the tool-error path");
        };
        assert!(results.contains("=== tool_error: list_dir ==="));
        assert!(results.contains("invalid tool input:"));
        assert!(results.contains("escapes project root"));
    }

    #[test]
    fn invalid_search_scope_outside_root_becomes_tool_error() {
        let (_dir, root, registry) = temp_root();
        let outside = TempDir::new().unwrap();

        let outcome = run_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "needle".into(),
                path: Some(outside.path().display().to_string()),
            }],
            ToolSurface::RetrievalFirst,
            false,
        );

        let ToolRoundOutcome::Completed { results, .. } = outcome else {
            panic!("invalid search scope should stay in the tool-error path");
        };
        assert!(results.contains("=== tool_error: search_code ==="));
        assert!(results.contains("invalid tool input:"));
        assert!(results.contains("escapes project root"));
    }

    #[test]
    fn valid_read_search_and_list_still_work() {
        let (_dir, root, registry) = temp_root();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/main.rs"),
            "const NEEDLE: &str = \"needle\";\n",
        )
        .unwrap();

        let outcome = run_round(
            &root,
            &registry,
            vec![
                ToolInput::SearchCode {
                    query: "needle".into(),
                    path: Some("src".into()),
                },
                ToolInput::ListDir { path: "src".into() },
                ToolInput::ReadFile {
                    path: "src/main.rs".into(),
                },
            ],
            ToolSurface::RetrievalFirst,
            false,
        );

        let ToolRoundOutcome::Completed { results, .. } = outcome else {
            panic!("valid read/search/list calls should complete");
        };
        assert!(results.contains("=== tool_result: search_code ==="));
        assert!(results.contains("=== tool_result: list_dir ==="));
        assert!(results.contains("=== tool_result: read_file ==="));
    }

    #[test]
    fn gate_checks_happen_before_resolution() {
        let (_dir, root, registry) = temp_root();
        let outside = TempDir::new().unwrap();

        let outcome = run_round(
            &root,
            &registry,
            vec![ToolInput::ListDir {
                path: outside.path().display().to_string(),
            }],
            ToolSurface::RetrievalFirst,
            true,
        );

        let ToolRoundOutcome::Completed { results, .. } = outcome else {
            panic!("list_dir-before-search should stay in the tool-error path");
        };
        assert!(results.contains("=== tool_error: list_dir ==="));
        assert!(results.contains(LIST_DIR_BEFORE_SEARCH_BLOCKED));
        assert!(!results.contains("escapes project root"));
    }

    #[test]
    fn disallowed_tools_are_rejected_before_resolution() {
        let (_dir, root, registry) = temp_root();
        let outside = TempDir::new().unwrap();

        let outcome = run_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: outside.path().join("outside.txt").display().to_string(),
            }],
            ToolSurface::AnswerOnly,
            false,
        );

        let ToolRoundOutcome::Completed { results, .. } = outcome else {
            panic!("disallowed read should stay in the tool-error path");
        };
        assert!(results.contains("=== tool_error: read_file ==="));
        assert!(results.contains(surface_policy_correction(ToolSurface::AnswerOnly)));
        assert!(!results.contains("invalid tool input:"));
    }

    #[test]
    fn non_candidate_read_dispatches_to_preferred_candidate() {
        // When the model reads a file not in the search results and a valid candidate
        // is available, the runtime dispatches the candidate directly instead of
        // injecting a correction and waiting for the model to retry.
        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("candidate.rs"), "fn needle() {}\n").unwrap();
        fs::write(root.path().join("other.rs"), "fn unrelated() {}\n").unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        // Round 1: search populates candidate list with candidate.rs
        run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "needle".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        assert!(
            investigation.search_produced_results(),
            "search must have found candidate.rs"
        );

        // Round 2: model attempts to read other.rs (not a search candidate).
        // Runtime must dispatch candidate.rs directly — no correction, no search reopen.
        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "other.rs".into(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
            panic!("non-candidate read must dispatch the preferred candidate");
        };
        let ToolInput::ReadFile { path } = call else {
            panic!("dispatched call must be read_file, not search_code");
        };
        assert_eq!(
            path, "candidate.rs",
            "dispatch must target the preferred candidate"
        );
    }

    #[test]
    fn non_candidate_read_correction_fallback_when_candidate_already_read() {
        // When the preferred candidate was already read this turn, dispatch is unsafe
        // (read would be a dedup-blocked duplicate). The runtime must fall back to
        // the correction path rather than dispatch.
        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("candidate.rs"), "fn needle() {}\n").unwrap();
        fs::write(root.path().join("other.rs"), "fn unrelated() {}\n").unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        // Round 1: search populates candidate list with candidate.rs
        run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "needle".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        // Round 2: model reads the candidate (valid — puts it in reads_this_turn)
        run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "candidate.rs".into(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        assert!(
            reads_this_turn.contains("candidate.rs"),
            "candidate.rs must be recorded as read this turn"
        );

        // Round 3: model reads other.rs (non-candidate). Dispatch is blocked because
        // candidate.rs is already in reads_this_turn. Must fall back to correction.
        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "other.rs".into(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::Completed {
            results: accumulated,
            ..
        } = outcome
        else {
            panic!("must fall back to correction, not dispatch or terminal");
        };
        assert!(
            accumulated.contains("`other.rs` was not returned by the search"),
            "correction must explain why the read was rejected: {accumulated}"
        );
        assert!(
            accumulated.contains("[read_file: candidate.rs]"),
            "correction must still name the best candidate: {accumulated}"
        );
    }

    #[test]
    fn non_candidate_read_repeated_offense_still_terminates() {
        // Even with Phase 18.1, a second non-candidate read after dispatch must terminate.
        // Candidate enforcement is not weakened — the runtime does not allow infinite
        // non-candidate reads to be silently redirected.
        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("candidate.rs"), "fn needle() {}\n").unwrap();
        fs::write(root.path().join("other.rs"), "fn unrelated() {}\n").unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "needle".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        // First offense: runtime dispatches candidate.rs
        let first = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "other.rs".into(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );
        assert!(
            matches!(first, ToolRoundOutcome::RuntimeDispatch { .. }),
            "first offense must dispatch"
        );

        // Second offense: attempts == 2 → terminal, regardless of candidates
        let second = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "other.rs".into(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );
        assert!(
            matches!(
                second,
                ToolRoundOutcome::TerminalAnswer {
                    reason: RuntimeTerminalReason::ReadFileFailed,
                    ..
                }
            ),
            "second non-candidate read offense must terminate"
        );
    }

    #[test]
    fn general_readme_candidate_first_read_redirects_to_source_candidate() {
        let (_dir, root, registry) = temp_root();
        fs::create_dir_all(root.path().join("sandbox/services")).unwrap();
        fs::write(
            root.path().join("sandbox/README.md"),
            "completed tasks are documented here.\n",
        )
        .unwrap();
        fs::write(
            root.path().join("sandbox/services/task_service.py"),
            "def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]\n",
        )
        .unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "completed".into(),
                path: Some("sandbox/".into()),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            Some("sandbox/"),
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        assert!(
            investigation.search_produced_results(),
            "search must have found README and source candidates"
        );

        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: "sandbox/README.md".into(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            Some("sandbox/"),
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
            panic!("README first-read should be redirected to the source candidate");
        };
        let ToolInput::ReadFile { path } = call else {
            panic!("redirected call must be read_file");
        };
        assert!(
            path.contains("sandbox/services/task_service.py"),
            "redirect must target the source candidate, got: {path}"
        );
    }

    #[test]
    fn definition_site_dispatch_accepted_on_usage_lookup() {
        // After usage candidates are exhausted on a UsageLookup, the runtime dispatches
        // the definition-site file (definition_after_usage_exhausted). Gate 1 must not
        // reject that read — the bypass must fire and accept it as evidence.
        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("usage.rs"), "let x = needle(args);\n").unwrap();
        fs::write(root.path().join("definition.rs"), "fn needle() {}\n").unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        // Round 1: search — UsageLookup immediately dispatches the preferred usage candidate
        let after_search = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "needle".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::UsageLookup,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::RuntimeDispatch { call, .. } = after_search else {
            panic!("search on UsageLookup must dispatch the preferred usage candidate");
        };
        let ToolInput::ReadFile { path: usage_path } = call else {
            panic!("dispatch must be read_file");
        };
        assert_eq!(
            usage_path, "usage.rs",
            "preferred candidate must be usage.rs"
        );

        // Round 2: read usage.rs — evidence satisfied; runtime then dispatches definition.rs
        let after_usage_read = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: usage_path.clone(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::UsageLookup,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::RuntimeDispatch { call, .. } = after_usage_read else {
            panic!("after usage read, runtime must dispatch the definition site candidate");
        };
        let ToolInput::ReadFile { path: def_path } = call else {
            panic!("dispatch must be read_file");
        };
        assert_eq!(
            def_path, "definition.rs",
            "definition-site dispatch must target definition.rs"
        );

        // Round 3: read definition.rs — bypass must accept it without triggering Gate 1
        let after_def_read = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::ReadFile {
                path: def_path.clone(),
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::UsageLookup,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        assert!(
            matches!(after_def_read, ToolRoundOutcome::Completed { .. }),
            "definition-site read must complete without Gate 1 cascade"
        );
        assert!(
            investigation.evidence_ready(),
            "evidence must be ready after reading the usage candidate"
        );
    }

    #[test]
    fn lsp_definition_seeded_on_definition_lookup_after_search() {
        // On a DefinitionLookup turn with lsp.enabled=true, the runtime must dispatch
        // lsp_definition to the top definition candidate immediately after search returns
        // results — without waiting for the model to emit a block-format call.
        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("lib.rs"), "fn target_fn() {}\n").unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut lsp = LspManager::new(
            &LspConfig {
                enabled: true,
                ..LspConfig::default()
            },
            std::path::Path::new("."),
        );
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "target_fn".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut lsp,
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::DefinitionLookup,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
            panic!("DefinitionLookup after search must seed lsp_definition dispatch");
        };
        assert!(
            matches!(call, ToolInput::LspDefinition { .. }),
            "dispatched call must be lsp_definition, got: {call:?}"
        );
        if let ToolInput::LspDefinition { path, line, col } = call {
            assert_eq!(
                path, "lib.rs",
                "lsp_definition path must be the definition candidate"
            );
            assert!(line >= 1, "line must be 1-based and >= 1");
            assert!(col >= 1, "col must be 1-based and >= 1");
        }
    }

    #[test]
    fn lsp_definition_seeded_prefers_declaration_line() {
        // Two matches in the same file: comment first (line 1), struct declaration second (line 2).
        // The seeded lsp_definition must use the struct declaration line, not the comment.
        let (_dir, root, registry) = temp_root();
        fs::write(
            root.path().join("lib.rs"),
            "// InvestigationGraph here\npub struct InvestigationGraph {}\n",
        )
        .unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut lsp = LspManager::new(
            &LspConfig {
                enabled: true,
                ..LspConfig::default()
            },
            std::path::Path::new("."),
        );
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "InvestigationGraph".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut lsp,
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::DefinitionLookup,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
            panic!("DefinitionLookup after search must seed lsp_definition dispatch");
        };
        let ToolInput::LspDefinition { path, line, col } = call else {
            panic!("dispatched call must be lsp_definition");
        };
        assert_eq!(path, "lib.rs");
        assert_eq!(
            line, 2,
            "lsp_definition must use the declaration line (2), not the comment line (1)"
        );
        assert!(col >= 1);
    }

    #[test]
    fn hover_not_injected_when_lsp_disabled() {
        // With LspManager constructed with enabled: false, a successful lsp_definition
        // result must not produce any lsp_hover block in the accumulated output.
        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("lib.rs"), "pub fn target_fn() {}\n").unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        // LSP disabled — hover must not fire even if lsp_definition result has a target.
        let mut lsp = LspManager::new(&LspConfig::default(), std::path::Path::new("."));
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        // Dispatch lsp_definition directly (skip seeding; use the intercept path).
        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::LspDefinition {
                path: "lib.rs".into(),
                line: 1,
                col: 1,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut lsp,
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            false,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        let ToolRoundOutcome::Completed { results, .. } = outcome else {
            panic!("lsp_definition dispatch must complete");
        };
        assert!(
            !results.contains("lsp_hover"),
            "no hover block must appear when LSP is disabled: {results}"
        );
    }

    #[test]
    fn lsp_definition_not_seeded_for_python_file() {
        // DefinitionLookup + LSP enabled must NOT seed LspDefinition when the
        // definition candidate is a non-Rust file — rust-analyzer returns empty
        // results for .py paths, which previously caused a recovery loop.
        let (_dir, root, registry) = temp_root();
        fs::write(
            root.path().join("module.py"),
            "def my_symbol(x):\n    pass\n",
        )
        .unwrap();

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut lsp = LspManager::new(
            &LspConfig {
                enabled: true,
                ..Default::default()
            },
            std::path::Path::new("."),
        );
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;

        let outcome = run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "my_symbol".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut lsp,
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            true,
            InvestigationMode::DefinitionLookup,
            None,
            &mut requested_read_completed,
            None,
            None,
            None,
            &RetrievalConfig::default(),
            &mut |_| {},
        );

        assert!(
            !matches!(
                outcome,
                ToolRoundOutcome::RuntimeDispatch {
                    call: ToolInput::LspDefinition { .. },
                    ..
                }
            ),
            "LSP seeding must be skipped for non-Rust (.py) definition candidates"
        );
    }

    #[test]
    fn agent_turn_search_emits_hybrid_search_decision_with_provider() {
        use crate::core::error::Result;
        use crate::runtime::index::EmbeddingProvider;
        use crate::runtime::trace::RUNTIME_TRACE_ENV;
        use crate::storage::index::SymbolStore;
        use crate::storage::session::schema;
        use rusqlite::Connection;

        struct ConstantEmbedProvider;
        impl EmbeddingProvider for ConstantEmbedProvider {
            fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
                Ok(texts.iter().map(|_| vec![0.1f32; 4]).collect())
            }
        }

        let (_dir, root, registry) = temp_root();
        fs::write(root.path().join("a.rs"), "fn needle_38() {}\n").unwrap();

        // Initialize schema so embedding_count() can query index_embeddings.
        let db_path = root.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        schema::initialize(&conn).unwrap();
        drop(conn);
        let store = SymbolStore::open(&db_path).unwrap();

        let provider = ConstantEmbedProvider;
        let retrieval_config = RetrievalConfig {
            vector_weight: 0.5,
            embedding_model: None,
        };

        // Enable tracing so trace_runtime_decision emits RuntimeTrace events.
        std::env::set_var(RUNTIME_TRACE_ENV, "1");

        let mut last_call_key = None;
        let mut search_budget = SearchBudget::new();
        let mut investigation = InvestigationState::new();
        let mut lsp = LspManager::new(&LspConfig::default(), std::path::Path::new("."));
        let mut reads_this_turn = HashSet::new();
        let mut anchors = AnchorState::default();
        let mut requested_read_completed = false;
        let mut disallowed = 0usize;
        let mut weak_query = 0usize;
        let mut events: Vec<crate::runtime::types::RuntimeEvent> = Vec::new();

        run_tool_round(
            &root,
            &registry,
            vec![ToolInput::SearchCode {
                query: "needle_38".into(),
                path: None,
            }],
            &mut last_call_key,
            &mut search_budget,
            &mut investigation,
            &mut lsp,
            &mut reads_this_turn,
            &mut anchors,
            ToolSurface::RetrievalFirst,
            &mut disallowed,
            &mut weak_query,
            false,
            false,
            InvestigationMode::General,
            None,
            &mut requested_read_completed,
            None,
            Some(&store),
            Some(&provider as &(dyn EmbeddingProvider + Send)),
            &retrieval_config,
            &mut |e| events.push(e),
        );

        std::env::remove_var(RUNTIME_TRACE_ENV);

        let has_hybrid_decision_true = events.iter().any(|e| {
            if let crate::runtime::types::RuntimeEvent::RuntimeTrace(line) = e {
                line.contains("event=hybrid_search_decision") && line.contains("use_vector=true")
            } else {
                false
            }
        });
        assert!(
            has_hybrid_decision_true,
            "agent-turn search_code with embedding_provider set must emit \
             hybrid_search_decision(use_vector=true); events: {events:?}"
        );
    }
}
