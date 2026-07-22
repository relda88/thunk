use crate::core::config::InvestigationDepth;
use crate::llm::backend::{BackendEvent, GenerateRequest, Message, Role};
use crate::tools::{
    PendingApprovalStage, PendingTransaction, ToolError, ToolInput, ToolOutput, ToolRunResult,
};

use super::super::super::investigation::prompt_analysis::looks_like_file_path;
use super::super::super::protocol::abilities::AbilityLoader;
use super::super::super::protocol::agent_prompts;
use super::super::super::protocol::plan_parser::{parse_plan, PlanStep};
use super::super::super::protocol::skills::SkillLoader;
use super::super::super::protocol::tool_codec;
use super::super::super::resolve;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{Activity, RuntimeEvent};
use super::super::telemetry::TurnPerformance;
use super::super::turn_state::PendingRuntimeCall;
use super::Runtime;

pub(crate) struct PendingPlanDraft {
    pub goal: String,
    pub steps: Vec<PlanStep>,
}

/// Bounds for /history output. Limits messages shown and chars per message to
/// prevent unbounded InfoMessage output from long or tool-heavy sessions.
const MAX_HISTORY_MESSAGES: usize = 10;
const MAX_MESSAGE_CHARS: usize = 200;

/// Explicit allowlist of tools that slash commands may invoke via the runtime.
/// All command-to-registry dispatch passes through this type — no command handler
/// calls registry.dispatch() directly or constructs ToolInput outside this enum.
/// Mutating tools are excluded by omission; adding one requires an explicit variant.
pub(super) enum CommandTool {
    ReadFile {
        path: String,
    },
    SearchCode {
        query: String,
    },
    GitBranch,
    GitStatus,
    GitDiff,
    GitLog,
    GitBranchCreate {
        name: String,
        start_point: Option<String>,
    },
    GitBranchSwitch {
        name: String,
    },
    ListDir {
        path: String,
    },
}

impl CommandTool {
    pub(super) fn into_input(self) -> ToolInput {
        match self {
            Self::ReadFile { path } => ToolInput::ReadFile { path },
            Self::SearchCode { query } => ToolInput::SearchCode { query, path: None },
            Self::GitBranch => ToolInput::GitBranch,
            Self::GitStatus => ToolInput::GitStatus,
            Self::GitDiff => ToolInput::GitDiff,
            Self::GitLog => ToolInput::GitLog,
            Self::GitBranchCreate { name, start_point } => {
                ToolInput::GitBranchCreate { name, start_point }
            }
            Self::GitBranchSwitch { name } => ToolInput::GitBranchSwitch { name },
            Self::ListDir { path } => ToolInput::ListDir { path },
        }
    }

    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::ReadFile { .. } => "read_file",
            Self::SearchCode { .. } => "search_code",
            Self::GitBranch => "git_branch",
            Self::GitStatus => "git_status",
            Self::GitDiff => "git_diff",
            Self::GitLog => "git_log",
            Self::GitBranchCreate { .. } => "git_branch_create",
            Self::GitBranchSwitch { .. } => "git_branch_switch",
            Self::ListDir { .. } => "list_dir",
        }
    }
}

impl Runtime {
    pub(super) fn handle_query_last(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let text = match self.conversation.last_assistant_content() {
            Some(content) => content.to_string(),
            None => "No previous response.".to_string(),
        };
        on_event(RuntimeEvent::InfoMessage(text));
    }

    pub(super) fn handle_query_anchors(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let mut parts = Vec::new();
        if let Some(path) = self.anchors.last_read_file() {
            parts.push(format!("last read:   {path}"));
        }
        if let Some((query, scope)) = self.anchors.last_search() {
            match scope {
                Some(s) => parts.push(format!("last search: {query} (in {s})")),
                None => parts.push(format!("last search: {query}")),
            }
        }
        let text = if parts.is_empty() {
            "no anchors set".to_string()
        } else {
            parts.join("\n")
        };
        on_event(RuntimeEvent::InfoMessage(text));
    }

    pub(super) fn handle_query_history(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let messages = self.conversation.human_visible_snapshot();

        if messages.is_empty() {
            on_event(RuntimeEvent::InfoMessage(
                "no conversation history".to_string(),
            ));
            return;
        }

        let tail = if messages.len() > MAX_HISTORY_MESSAGES {
            messages[messages.len() - MAX_HISTORY_MESSAGES..].to_vec()
        } else {
            messages
        };

        let mut lines = vec!["history:".to_string()];
        let mut first = true;
        for msg in &tail {
            let label = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => continue,
            };
            if msg.role == Role::User && !first {
                lines.push(String::new());
            }
            let content = if msg.content.chars().count() > MAX_MESSAGE_CHARS {
                let truncated: String = msg.content.chars().take(MAX_MESSAGE_CHARS).collect();
                format!("{truncated}...")
            } else {
                msg.content.clone()
            };
            lines.push(format!("[{label}] {content}"));
            first = false;
        }

        on_event(RuntimeEvent::InfoMessage(lines.join("\n")));
    }

    pub(super) fn dispatch_command_tool(
        &mut self,
        tool: CommandTool,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::Failed {
                message: "cannot run command while a tool approval is pending".to_string(),
            });
            return;
        }
        let search_query = match &tool {
            CommandTool::SearchCode { query } => Some(query.clone()),
            CommandTool::ReadFile { .. }
            | CommandTool::GitBranch
            | CommandTool::GitStatus
            | CommandTool::GitDiff
            | CommandTool::GitLog
            | CommandTool::GitBranchCreate { .. }
            | CommandTool::GitBranchSwitch { .. }
            | CommandTool::ListDir { .. } => None,
        };
        let name = tool.name();
        let input = tool.into_input();
        let resolved = match resolve(&self.project_root, &input) {
            Ok(resolved) => resolved,
            Err(error) => {
                let tool_error: ToolError = error.into();
                on_event(RuntimeEvent::InfoMessage(format!("error: {}", tool_error)));
                return;
            }
        };
        match self.registry.dispatch(resolved) {
            Ok(ToolRunResult::Immediate(output)) => {
                self.anchors.record_successful_read(&output);
                if let Some(query) = search_query {
                    self.anchors.record_successful_search(&output, query, None);
                }
                on_event(RuntimeEvent::InfoMessage(tool_codec::format_tool_result(
                    name, &output,
                )));
            }
            Ok(ToolRunResult::Approval(pending)) => {
                self.pending_action = Some(PendingApprovalStage::AwaitingPreCheck(
                    PendingTransaction::single(pending.clone()),
                ));
                on_event(RuntimeEvent::ApprovalRequired {
                    pending,
                    evidence: vec![],
                    impact: vec![],
                    reason: None,
                });
            }
            Err(e) => {
                on_event(RuntimeEvent::InfoMessage(format!("error: {e}")));
            }
        }
    }

    pub(super) fn handle_read_file(
        &mut self,
        path: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let p = std::path::Path::new(&path);
        if p.is_absolute() {
            on_event(RuntimeEvent::InfoMessage(
                "error: path must be relative".to_string(),
            ));
            return;
        }
        if p.components().any(|c| c == std::path::Component::ParentDir) {
            on_event(RuntimeEvent::InfoMessage(
                "error: path must not contain '..' components".to_string(),
            ));
            return;
        }
        self.dispatch_command_tool(CommandTool::ReadFile { path }, on_event);
    }

    pub(super) fn handle_git_branch(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.dispatch_command_tool(CommandTool::GitBranch, on_event);
    }

    pub(super) fn handle_git_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.dispatch_command_tool(CommandTool::GitStatus, on_event);
    }

    pub(super) fn handle_git_diff(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.dispatch_command_tool(CommandTool::GitDiff, on_event);
    }

    pub(super) fn handle_git_log(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.dispatch_command_tool(CommandTool::GitLog, on_event);
    }

    pub(super) fn handle_branch_create(
        &mut self,
        name: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        self.dispatch_command_tool(
            CommandTool::GitBranchCreate {
                name,
                start_point: None,
            },
            on_event,
        );
    }

    pub(super) fn handle_branch_switch(
        &mut self,
        name: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        // git checkout itself refuses to switch with uncommitted conflicting changes —
        // no pre-check needed here; errors propagate via the execute_approved failure path.
        self.dispatch_command_tool(CommandTool::GitBranchSwitch { name }, on_event);
    }

    /// Generates a conventional commit message via a one-shot backend call.
    /// Does not touch self.conversation — messages are assembled from a pruned
    /// snapshot and passed directly to the backend without being stored.
    fn generate_commit_message(&mut self, diff_context: &str) -> Option<String> {
        let prompt = format!(
            "You are generating a git commit message. Respond with \
             ONLY the commit message text — no explanation, no markdown, no preamble.\n\n\
             Format: <type>(<scope>): <description>\n\
             Types: feat, fix, docs, refactor, test, chore\n\
             Keep the subject line under 72 characters.\n\
             If needed, add a blank line then a body.\n\n\
             Staged changes:\n{diff_context}"
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
            return None;
        }
        let msg = result.trim().to_string();
        if msg.is_empty() {
            None
        } else {
            Some(msg)
        }
    }

    pub(super) fn handle_diff(
        &mut self,
        mode: crate::runtime::types::DiffMode,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        use crate::runtime::types::DiffMode;

        let base_ref: Option<String> = match mode {
            DiffMode::WorkingTree => None,
            DiffMode::SessionStart => match &self.session_start_ref {
                Some(r) => Some(r.clone()),
                None => {
                    on_event(RuntimeEvent::SystemMessage(
                        "diff last: no session baseline (no commits at session start)".into(),
                    ));
                    return;
                }
            },
            DiffMode::Ref(r) => Some(r),
        };

        let tool = crate::tools::GitDiffTool::new(self.project_root.as_path_buf());
        let output = tool.run_with_ref(base_ref.as_deref());

        match output {
            Ok(diff) => {
                let rendered =
                    tool_codec::format_tool_result("git_diff", &ToolOutput::GitDiff(diff.clone()));
                on_event(RuntimeEvent::InfoMessage(rendered));
                if diff.truncated {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "diff truncated at {} bytes",
                        diff.bytes_shown
                    )));
                }
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!("diff failed: {e}")));
            }
        }
    }

    pub(super) fn handle_commit(
        &mut self,
        message: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::Failed {
                message: "cannot commit while a tool approval is pending".to_string(),
            });
            return;
        }

        // Check for staged changes.
        let status_resolved = match resolve(&self.project_root, &ToolInput::GitStatus) {
            Ok(r) => r,
            Err(error) => {
                let tool_error: ToolError = error.into();
                on_event(RuntimeEvent::InfoMessage(format!(
                    "commit: git status failed: {tool_error}"
                )));
                return;
            }
        };
        let has_staged = match self.registry.dispatch(status_resolved) {
            Ok(ToolRunResult::Immediate(ToolOutput::GitStatus(s))) => s.entries.iter().any(|e| {
                let x = e.xy.chars().next().unwrap_or(' ');
                x != ' ' && x != '?'
            }),
            _ => false,
        };
        if !has_staged {
            on_event(RuntimeEvent::SystemMessage(
                "nothing to commit (no staged changes)".to_string(),
            ));
            return;
        }

        // Get the staged diff for commit message generation context.
        let diff_context = match resolve(&self.project_root, &ToolInput::GitDiffStaged).ok() {
            Some(resolved) => match self.registry.dispatch(resolved) {
                Ok(ToolRunResult::Immediate(ToolOutput::GitDiffStaged(d)))
                    if !d.patch.is_empty() =>
                {
                    if d.patch.len() > 4000 {
                        format!("{}... (truncated)", &d.patch[..4000])
                    } else {
                        d.patch.clone()
                    }
                }
                _ => "(binary or empty staged diff)".to_string(),
            },
            None => "(binary or empty staged diff)".to_string(),
        };

        // Use provided message or generate via model.
        let commit_message = match message {
            Some(m) if !m.trim().is_empty() => m,
            _ => match self.generate_commit_message(&diff_context) {
                Some(m) => m,
                None => {
                    on_event(RuntimeEvent::SystemMessage(
                        "commit: failed to generate message \
                         — use /commit \"your message\" to provide one manually"
                            .to_string(),
                    ));
                    return;
                }
            },
        };

        // Dispatch git_commit to produce the PendingAction, then surface for approval.
        let commit_resolved = match resolve(
            &self.project_root,
            &ToolInput::GitCommit {
                message: commit_message,
            },
        ) {
            Ok(r) => r,
            Err(error) => {
                let tool_error: ToolError = error.into();
                on_event(RuntimeEvent::InfoMessage(format!("commit: {tool_error}")));
                return;
            }
        };
        match self.registry.dispatch(commit_resolved) {
            Ok(ToolRunResult::Approval(pending)) => {
                self.pending_action = Some(PendingApprovalStage::AwaitingPreCheck(
                    PendingTransaction::single(pending.clone()),
                ));
                on_event(RuntimeEvent::ApprovalRequired {
                    pending,
                    evidence: vec![],
                    impact: vec![],
                    reason: None,
                });
            }
            Ok(ToolRunResult::Immediate(_)) => {
                // git_commit is RequiresApproval — an Immediate result is a bug.
            }
            Err(e) => {
                on_event(RuntimeEvent::InfoMessage(format!("commit: {e}")));
            }
        }
    }

    pub(super) fn handle_list_dir(&mut self, path: String, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.dispatch_command_tool(CommandTool::ListDir { path }, on_event);
    }

    pub(super) fn handle_search_code(
        &mut self,
        query: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if query.trim().len() < 2 {
            on_event(RuntimeEvent::InfoMessage(
                "error: search query must be at least 2 characters".to_string(),
            ));
            return;
        }

        let use_vector = self.retrieval_config.vector_weight > 0.0
            && self.embedding_provider.is_some()
            && self.symbol_store.is_some();

        if !use_vector {
            let reason = if self.retrieval_config.vector_weight <= 0.0 {
                "vec_weight_zero"
            } else if self.embedding_provider.is_none() {
                "no_provider"
            } else {
                "no_store"
            };
            trace_runtime_decision(
                on_event,
                "hybrid_search_decision",
                &[("use_vector", "false".into()), ("reason", reason.into())],
            );
            self.dispatch_command_tool(CommandTool::SearchCode { query }, on_event);
            return;
        }
        trace_runtime_decision(
            on_event,
            "hybrid_search_decision",
            &[("use_vector", "true".into()), ("reason", "enabled".into())],
        );

        // Keyword search — run directly so we can capture the output before augmenting.
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::Failed {
                message: "cannot run command while a tool approval is pending".to_string(),
            });
            return;
        }

        let input = ToolInput::SearchCode {
            query: query.clone(),
            path: None,
        };
        let resolved = match resolve(&self.project_root, &input) {
            Ok(r) => r,
            Err(e) => {
                let te: ToolError = e.into();
                on_event(RuntimeEvent::InfoMessage(format!("error: {te}")));
                return;
            }
        };
        let keyword_output = match self.registry.dispatch(resolved) {
            Ok(ToolRunResult::Immediate(out)) => out,
            Ok(ToolRunResult::Approval(_)) => {
                // search_code is always Immediate; fall back if this ever changes.
                self.dispatch_command_tool(CommandTool::SearchCode { query }, on_event);
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::InfoMessage(format!("error: {e}")));
                return;
            }
        };

        // Preserve anchor recording — invariant from dispatch_command_tool:188-191.
        self.anchors.record_successful_read(&keyword_output);
        self.anchors
            .record_successful_search(&keyword_output, query.clone(), None);

        // Try to augment with vector results (discard was_applied — not an investigation turn).
        let (final_output, _) = super::super::tool_round::try_vector_augment(
            &query,
            keyword_output,
            self.symbol_store.as_ref(),
            self.embedding_provider.as_deref(),
            &self.retrieval_config,
            &self.project_root,
            on_event,
        );
        on_event(RuntimeEvent::InfoMessage(tool_codec::format_tool_result(
            "search_code",
            &final_output,
        )));
    }

    pub(super) fn handle_reset(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.pending_action = None;
        self.pending_plan = None;
        self.pending_memory = None;
        self.pending_memory_queue.clear();
        self.pending_embed = None;
        self.active_sequence_id = None;
        // exec mode is session-scoped — a reset returns it to the default-deny state.
        self.exec_enabled = false;
        // do-not-disturb is session-scoped — a reset returns it to the default off state.
        self.dnd_enabled = false;
        // proactive cadence is session-scoped — clear the floor so the next session can scan.
        self.last_proactive_at = None;
        self.anchors.clear();
        trace_runtime_decision(
            on_event,
            "anchor_cleared",
            &[("kind", "last_read_file".into())],
        );
        trace_runtime_decision(
            on_event,
            "anchor_cleared",
            &[("kind", "last_search".into())],
        );
        self.context_75_warned = false;
        self.conversation.reset(self.system_prompt.clone());
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    pub(super) fn handle_undo(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        match self.undo_stack.pop() {
            None => {
                on_event(RuntimeEvent::SystemMessage("Nothing to undo.".to_string()));
            }
            Some((path, contents)) => {
                if contents.is_empty() {
                    let _ = std::fs::remove_file(&path);
                } else {
                    let _ = std::fs::write(&path, &contents);
                }
                on_event(RuntimeEvent::SystemMessage(format!(
                    "Undone: restored {}",
                    path
                )));
            }
        }
    }

    pub(super) fn handle_lsp_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let report = self.lsp.health_report();
        on_event(RuntimeEvent::SystemMessage(report));
    }

    /// Lists configured MCP servers with liveness and discovered tool counts.
    pub(super) fn handle_mcp_list(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let Some(ref mut mgr) = self.mcp_manager else {
            on_event(RuntimeEvent::SystemMessage(
                "No MCP servers configured".to_string(),
            ));
            return;
        };
        let names: Vec<String> = mgr.server_names().map(|s| s.to_string()).collect();
        let mut lines = vec!["MCP servers:".to_string()];
        for name in &names {
            let alive = mgr.is_alive(name);
            let marker = if alive { '●' } else { '○' };
            let status = if alive { "alive" } else { "dead" };
            let command = mgr
                .server_config(name)
                .map(|c| c.command.clone())
                .unwrap_or_default();
            let tool_count = self
                .discovered_tools
                .iter()
                .filter(|t| &t.server_name == name)
                .count();
            lines.push(format!("{marker} {name} ({status})"));
            lines.push(format!("  command: {command}"));
            lines.push(format!("  tools: {tool_count} discovered"));
        }
        on_event(RuntimeEvent::SystemMessage(lines.join("\n")));
    }

    /// Re-runs MCP tool discovery and refreshes the dynamic tool set.
    pub(super) fn handle_mcp_refresh(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if self.mcp_manager.is_none() {
            on_event(RuntimeEvent::SystemMessage(
                "No MCP servers configured".to_string(),
            ));
            return;
        }
        // Mirror Runtime::new()'s collision filter: a namespaced MCP tool whose name
        // collides with a static tool name is skipped with a warning.
        let specs = self.registry.specs();
        let static_names: std::collections::HashSet<&str> = specs.iter().map(|s| s.name).collect();
        let discovered: Vec<crate::runtime::mcp::McpTool> = self
            .mcp_manager
            .as_mut()
            .map(|m| m.discover_all())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| {
                if static_names.contains(t.name.as_str()) {
                    eprintln!("MCP tool name collision: {}, skipping", t.name);
                    false
                } else {
                    true
                }
            })
            .collect();
        let tool_count = discovered.len();
        let server_count = self
            .mcp_manager
            .as_ref()
            .map(|m| m.server_names().count())
            .unwrap_or(0);
        self.discovered_tools = discovered;
        // The parser and surface layers recompute from self.discovered_tools each turn,
        // so they pick up the refreshed set automatically on the next turn. The system
        // prompt is built once at construction and is NOT rebuilt here — its advertised
        // tool list may be stale until the session restarts (v1 limitation).
        on_event(RuntimeEvent::SystemMessage(format!(
            "MCP tools refreshed: {tool_count} tools discovered across {server_count} servers"
        )));
    }

    pub(super) fn handle_index_build(
        &mut self,
        large: bool,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.symbol_store.is_none() {
            on_event(RuntimeEvent::SystemMessage(
                "index: not available (no db path)".to_string(),
            ));
            return;
        }
        let mode = if large { " (large)" } else { "" };
        on_event(RuntimeEvent::SystemMessage(format!(
            "index: building{mode}..."
        )));
        let symbols = crate::runtime::index::extract_symbols(&self.project_root);
        let count = symbols.len();
        let project_root = self.project_root.path().to_string_lossy().to_string();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if let Some(ref store) = self.symbol_store {
            if let Err(e) = store.upsert_symbols(&project_root, &symbols) {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "index: build failed: {e}"
                )));
                return;
            }
            let imports = crate::runtime::index::extract_imports(&self.project_root);
            if let Err(e) = store.upsert_imports(&project_root, &imports) {
                trace_runtime_decision(
                    on_event,
                    "storage_warning",
                    &[("op", "upsert_imports".into()), ("err", e.to_string())],
                );
            }
            // Record build timestamp via the project-level sentinel row.
            if let Err(e) = store.upsert_file_metadata(&project_root, "", now_secs, "") {
                trace_runtime_decision(
                    on_event,
                    "storage_warning",
                    &[
                        ("op", "upsert_file_metadata".into()),
                        ("err", e.to_string()),
                    ],
                );
            }
            self.index_triggered = true;
            on_event(RuntimeEvent::SystemMessage(format!(
                "index: {count} symbols indexed"
            )));
        }
    }

    pub(super) fn handle_index_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let project_root = self.project_root.path().to_string_lossy().to_string();
        let Some(ref store) = self.symbol_store else {
            on_event(RuntimeEvent::SystemMessage(
                "index: not available (no db path)".to_string(),
            ));
            return;
        };
        let sym_count = store.symbol_count(&project_root).unwrap_or(0);
        let imp_count = store.import_count(&project_root).unwrap_or(0);
        let last_build = store
            .last_build_time(&project_root)
            .ok()
            .flatten()
            .map(|ts| {
                // ts is Unix seconds — format as a human-readable value.
                format!("{ts}s since epoch")
            })
            .unwrap_or_else(|| "never".to_string());
        on_event(RuntimeEvent::SystemMessage(format!(
            "index: {sym_count} symbols, {imp_count} imports, last build: {last_build}"
        )));
    }

    pub(super) fn handle_context_stats(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let token_estimate: usize = self
            .conversation
            .pruned_snapshot()
            .iter()
            .map(|m| m.content.len())
            .sum::<usize>()
            / 4;
        let msg_count = self.conversation.message_count();
        let tool_count = self.conversation.tool_result_count();
        let oldest = self
            .conversation
            .oldest_tool_result_turn_age()
            .map(|n| format!("{n} turns ago"))
            .unwrap_or_else(|| "none".to_string());
        let ctx_pct = self
            .backend
            .capabilities()
            .context_window_tokens
            .filter(|&ctx| ctx > 0)
            .map(|ctx| token_estimate * 100 / ctx as usize);

        let pct_str = ctx_pct
            .map(|p| format!(", context {p}%"))
            .unwrap_or_default();
        on_event(RuntimeEvent::SystemMessage(format!(
            "context: ~{token_estimate} tokens (estimated), {msg_count} messages, \
{tool_count} tool results, oldest {oldest}{pct_str}"
        )));
    }

    pub(super) fn handle_compact(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let count = self.conversation.compact_stale_tool_results();
        if count == 0 {
            on_event(RuntimeEvent::SystemMessage(
                "compact: nothing to compact".to_string(),
            ));
        } else {
            on_event(RuntimeEvent::SystemMessage(format!(
                "compact: {count} stale tool result{} pruned",
                if count == 1 { "" } else { "s" }
            )));
        }
    }

    pub(super) fn maybe_warn_or_prune_context(
        &mut self,
        perf: &TurnPerformance,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let Some(pct) = perf.context_used_pct() else {
            return;
        };
        if pct >= 90 {
            let count = self.conversation.compact_stale_tool_results();
            trace_runtime_decision(
                on_event,
                "context_compacted",
                &[("pct", pct.to_string()), ("count", count.to_string())],
            );
            if count > 0 {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "context at {pct}% — auto-compacted {count} stale tool result(s)"
                )));
            }
            self.context_75_warned = true;
        } else if pct >= 75 && !self.context_75_warned {
            self.context_75_warned = true;
            trace_runtime_decision(
                on_event,
                "context_warning_75pct",
                &[("pct", pct.to_string())],
            );
            on_event(RuntimeEvent::SystemMessage(
                "context at 75% — run /compact to free space".to_string(),
            ));
        }
    }

    /// Fires at most once per session: if the symbol index is empty after the first
    /// search operation, runs a synchronous index build and emits a status message.
    pub(super) fn maybe_trigger_index_build(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if self.index_triggered {
            return;
        }
        self.index_triggered = true;
        let project_root = self.project_root.path().to_string_lossy().to_string();
        let is_empty = match &self.symbol_store {
            Some(store) => store.is_empty(&project_root).unwrap_or(false),
            None => return,
        };
        if !is_empty {
            return;
        }
        on_event(RuntimeEvent::SystemMessage(
            "index: empty — building...".to_string(),
        ));
        let symbols = crate::runtime::index::extract_symbols(&self.project_root);
        let count = symbols.len();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if let Some(ref store) = self.symbol_store {
            match store.upsert_symbols(&project_root, &symbols) {
                Ok(()) => {
                    let imports = crate::runtime::index::extract_imports(&self.project_root);
                    if let Err(e) = store.upsert_imports(&project_root, &imports) {
                        trace_runtime_decision(
                            on_event,
                            "storage_warning",
                            &[("op", "upsert_imports".into()), ("err", e.to_string())],
                        );
                    }
                    if let Err(e) = store.upsert_file_metadata(&project_root, "", now_secs, "") {
                        trace_runtime_decision(
                            on_event,
                            "storage_warning",
                            &[
                                ("op", "upsert_file_metadata".into()),
                                ("err", e.to_string()),
                            ],
                        );
                    }
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "index: {count} symbols indexed"
                    )));
                }
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "index: build failed: {e}"
                    )));
                }
            }
        }
    }

    pub(super) fn rebuild_index_for_file(
        &mut self,
        abs_path: &std::path::Path,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let store = match &self.symbol_store {
            Some(s) => s,
            None => return,
        };
        let project_root = self.project_root.path().to_string_lossy().to_string();
        if store.is_empty(&project_root).unwrap_or(true) {
            return;
        }
        let file_path = match abs_path.strip_prefix(self.project_root.path()) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => return,
        };
        let symbols = crate::runtime::index::extract_symbols_for_file(abs_path, &self.project_root);
        let edges = crate::runtime::index::extract_imports_for_file(abs_path, &self.project_root);
        if let Err(e) = store.delete_embeddings_for_file(&project_root, &file_path) {
            trace_runtime_decision(
                on_event,
                "storage_warning",
                &[
                    ("op", "delete_embeddings_for_file".into()),
                    ("err", e.to_string()),
                ],
            );
        }
        if let Err(e) = store.upsert_symbols_for_file(&project_root, &file_path, &symbols) {
            trace_runtime_decision(
                on_event,
                "storage_warning",
                &[
                    ("op", "upsert_symbols_for_file".into()),
                    ("err", e.to_string()),
                ],
            );
        }
        if let Err(e) = store.upsert_imports_for_file(&project_root, &file_path, &edges) {
            trace_runtime_decision(
                on_event,
                "storage_warning",
                &[
                    ("op", "upsert_imports_for_file".into()),
                    ("err", e.to_string()),
                ],
            );
        }
        trace_runtime_decision(on_event, "index_rebuilt_for_file", &[("path", file_path)]);
    }

    pub(super) fn handle_providers_list(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let current = self.config.llm.provider.as_str();
        let providers = [
            ("llamacpp", "llama_cpp"),
            ("openai", "openai"),
            ("ollama", "ollama"),
            ("openrouter", "openrouter"),
            ("groq", "groq"),
        ];
        let mut lines = vec!["providers:".to_string()];
        for (display, internal) in &providers {
            let marker = if *internal == current {
                " (active)"
            } else {
                ""
            };
            lines.push(format!("  {}{}", display, marker));
        }
        on_event(RuntimeEvent::SystemMessage(lines.join("\n")));
    }

    pub(super) fn handle_providers_use(
        &mut self,
        name: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let normalized = match name.as_str() {
            "llamacpp" | "llama_cpp" => "llama_cpp",
            "openai" => "openai",
            "ollama" => "ollama",
            "openrouter" => "openrouter",
            "groq" => "groq",
            other => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "Unknown provider '{}'. Known: llamacpp, openai, ollama, openrouter, groq",
                    other
                )));
                return;
            }
        };
        let prev_provider = self.config.llm.provider.clone();
        let mut new_config = self.config.clone();
        new_config.llm.provider = normalized.to_string();
        match crate::llm::providers::build_backend(&new_config) {
            Ok(new_backend) => {
                self.backend = new_backend;
                self.config.llm.provider = normalized.to_string();
                trace_runtime_decision(
                    on_event,
                    "provider_switched",
                    &[("from", prev_provider), ("to", normalized.to_string())],
                );
                on_event(RuntimeEvent::SystemMessage(format!(
                    "Switched to provider: {}",
                    normalized
                )));
            }
            Err(e) => {
                trace_runtime_decision(
                    on_event,
                    "provider_switch_failed",
                    &[("name", normalized.to_string())],
                );
                on_event(RuntimeEvent::SystemMessage(format!(
                    "Failed to switch to '{}': {}",
                    normalized, e
                )));
            }
        }
    }

    pub(super) fn handle_prompt_physics_toggle(
        &mut self,
        enabled: Option<bool>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match enabled {
            Some(true) => {
                self.prompt_physics.enabled = true;
                trace_runtime_decision(
                    on_event,
                    "prompt_physics_toggled",
                    &[("enabled", "true".into())],
                );
                on_event(RuntimeEvent::SystemMessage(
                    "prompt physics: enabled".to_string(),
                ));
            }
            Some(false) => {
                self.prompt_physics.enabled = false;
                trace_runtime_decision(
                    on_event,
                    "prompt_physics_toggled",
                    &[("enabled", "false".into())],
                );
                on_event(RuntimeEvent::SystemMessage(
                    "prompt physics: disabled".to_string(),
                ));
            }
            None => {
                let status = if self.prompt_physics.enabled {
                    "prompt physics: enabled"
                } else {
                    "prompt physics: disabled"
                };
                on_event(RuntimeEvent::SystemMessage(status.to_string()));
            }
        }
    }

    pub(super) fn handle_constrained_output_toggle(
        &mut self,
        enabled: Option<bool>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match enabled {
            Some(true) => {
                self.constrained_output = true;
                trace_runtime_decision(
                    on_event,
                    "constrained_output_toggled",
                    &[("enabled", "true".into())],
                );
                on_event(RuntimeEvent::SystemMessage(
                    "constrained output: enabled".to_string(),
                ));
            }
            Some(false) => {
                self.constrained_output = false;
                trace_runtime_decision(
                    on_event,
                    "constrained_output_toggled",
                    &[("enabled", "false".into())],
                );
                on_event(RuntimeEvent::SystemMessage(
                    "constrained output: disabled".to_string(),
                ));
            }
            None => {
                let status = if self.constrained_output {
                    "constrained output: enabled"
                } else {
                    "constrained output: disabled"
                };
                on_event(RuntimeEvent::SystemMessage(status.to_string()));
            }
        }
    }

    pub(super) fn handle_compress_toggle(
        &mut self,
        enabled: Option<bool>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match enabled {
            Some(true) => {
                self.prompt_physics.compress_abilities = true;
                trace_runtime_decision(
                    on_event,
                    "prompt_compression_toggled",
                    &[("enabled", "true".into())],
                );
                let msg = if self.prompt_physics.active_ability.is_some() {
                    "compression: enabled"
                } else {
                    "compression: enabled (no active ability — no-op)"
                };
                on_event(RuntimeEvent::SystemMessage(msg.to_string()));
            }
            Some(false) => {
                self.prompt_physics.compress_abilities = false;
                trace_runtime_decision(
                    on_event,
                    "prompt_compression_toggled",
                    &[("enabled", "false".into())],
                );
                on_event(RuntimeEvent::SystemMessage(
                    "compression: disabled".to_string(),
                ));
            }
            None => {
                let status = if self.prompt_physics.compress_abilities {
                    if self.prompt_physics.active_ability.is_some() {
                        "compression: enabled"
                    } else {
                        "compression: enabled (no active ability — no-op)"
                    }
                } else {
                    "compression: disabled"
                };
                on_event(RuntimeEvent::SystemMessage(status.to_string()));
            }
        }
    }

    pub(super) fn handle_refactor(
        &mut self,
        target: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        use crate::llm::backend::{BackendEvent, GenerateRequest, Message};
        use crate::storage::tasks::{EditSequence, EditStep, SequenceStatus, StepStatus};
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        if self.edit_store.is_none() {
            on_event(RuntimeEvent::Failed {
                message: "Edit store not initialized".to_string(),
            });
            return;
        }

        let goal = target.unwrap_or_else(|| "refactor the codebase".to_string());

        let prompt = format!(
            "Decompose the following refactor goal into an ordered list of single-file edit steps.\n\
             Respond with a JSON array only. No prose, no markdown fences.\n\
             Format: [{{\"file\": \"path/to/file.rs\", \"description\": \"what changes and why\"}}, ...]\n\
             Limit to 10 steps maximum. Each step must target exactly one file.\n\n\
             Goal: {goal}"
        );

        let mut messages = self.conversation.pruned_snapshot();
        messages.push(Message::user(prompt));
        let request = GenerateRequest::new(messages);
        let mut raw = String::new();
        if self
            .backend
            .generate(request, &mut |event| {
                if let BackendEvent::TextDelta(chunk) = event {
                    raw.push_str(&chunk);
                }
            })
            .is_err()
        {
            on_event(RuntimeEvent::Failed {
                message: format!("refactor: model generation failed\n{raw}"),
            });
            return;
        }

        #[derive(serde::Deserialize)]
        struct RawStep {
            file: String,
            description: String,
        }

        let raw_steps: Vec<RawStep> = match serde_json::from_str(raw.trim()) {
            Ok(v) => v,
            Err(_) => {
                on_event(RuntimeEvent::Failed {
                    message: format!("refactor: failed to parse model response\n{}", raw.trim()),
                });
                return;
            }
        };

        fn gen_id() -> String {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let unique = nanos.wrapping_add(count) ^ (std::process::id() as u64);
            format!("{unique:016x}")
        }

        let seq_id = gen_id();
        let steps: Vec<EditStep> = raw_steps
            .iter()
            .enumerate()
            .map(|(i, s)| EditStep {
                id: gen_id(),
                sequence_id: seq_id.clone(),
                position: i,
                file: PathBuf::from(&s.file),
                search: String::new(),
                replace: String::new(),
                verification_cmd: None,
                status: StepStatus::Pending,
                generated: false,
            })
            .collect();

        const SIG_KEYWORDS: &[&str] = &["rename", "signature", "parameter", "argument"];
        let steps = {
            let mut steps = steps;
            let mut auto_steps: Vec<EditStep> = Vec::new();
            if let Some(store) = &self.symbol_store {
                let root = self.project_root.path().to_string_lossy().into_owned();
                for raw in &raw_steps {
                    let desc_lower = raw.description.to_lowercase();
                    if SIG_KEYWORDS.iter().any(|kw| desc_lower.contains(kw)) {
                        if let Ok(importers) = store.importers_of(&root, &raw.file, 5) {
                            for importer_file in importers {
                                auto_steps.push(EditStep {
                                    id: gen_id(),
                                    sequence_id: seq_id.clone(),
                                    position: steps.len() + auto_steps.len(),
                                    file: PathBuf::from(&importer_file),
                                    search: String::new(),
                                    replace: String::new(),
                                    verification_cmd: None,
                                    status: StepStatus::Pending,
                                    generated: true,
                                });
                            }
                        }
                    }
                }
            }
            let cap = 20usize.saturating_sub(steps.len());
            auto_steps.truncate(cap);
            steps.extend(auto_steps);
            steps
        };

        let steps = if let Some(store) = &self.symbol_store {
            let project_root_str = self.project_root.path().to_string_lossy().into_owned();
            match store.all_imports(&project_root_str) {
                Ok(edges) => toposort_steps(steps, &edges, self.project_root.path()),
                Err(_) => steps,
            }
        } else {
            steps
        };

        let unique_files: std::collections::HashSet<_> =
            raw_steps.iter().map(|s| &s.file).collect();
        let n = steps.len();
        let file_count = unique_files.len();

        let sequence = EditSequence {
            id: seq_id,
            task_id: None,
            goal: goal.clone(),
            steps,
            current_idx: 0,
            status: SequenceStatus::Planning,
            snapshot_ref: None,
        };

        if let Some(store) = &self.edit_store {
            if let Err(e) = store.create_sequence(&sequence) {
                on_event(RuntimeEvent::Failed {
                    message: format!("refactor: failed to persist sequence: {e}"),
                });
                return;
            }
        }

        on_event(RuntimeEvent::SystemMessage(format!(
            "Refactor sequence created: {n} steps across {file_count} file{}",
            if file_count == 1 { "" } else { "s" }
        )));
    }

    pub(super) fn handle_sequence_approve(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        use crate::storage::tasks::SequenceStatus;

        let store = match &self.edit_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::Failed {
                    message: "No edit store configured".to_string(),
                });
                return;
            }
        };

        let sequence_id = if let Some(id) = &self.active_sequence_id {
            id.clone()
        } else {
            match store.get_latest_sequence() {
                Ok(Some(seq)) => {
                    let id = seq.id.clone();
                    self.active_sequence_id = Some(id.clone());
                    id
                }
                Ok(None) => {
                    on_event(RuntimeEvent::Failed {
                        message: "No refactor sequence found".to_string(),
                    });
                    return;
                }
                Err(e) => {
                    on_event(RuntimeEvent::Failed {
                        message: format!("Failed to load sequence: {e}"),
                    });
                    return;
                }
            }
        };

        let store = match &self.edit_store {
            Some(s) => s,
            None => return,
        };

        if let Err(e) = store.update_sequence_status(&sequence_id, SequenceStatus::Approved) {
            on_event(RuntimeEvent::Failed {
                message: format!("Failed to approve sequence: {e}"),
            });
            return;
        }

        let summary = {
            let s = self.edit_store.as_ref().expect("already checked above");
            match s.get_sequence(&sequence_id) {
                Ok(Some(seq)) => {
                    let n = seq.steps.len();
                    let mut lines = vec![format!("Refactor sequence approved — {n} steps")];
                    for step in &seq.steps {
                        let badge = if step.generated { " [auto]" } else { "" };
                        lines.push(format!(
                            "{}. {}{}",
                            step.position + 1,
                            step.file.display(),
                            badge
                        ));
                    }
                    lines.push(String::new());
                    lines.push(
                        "Type /refactor status to check progress, /refactor abort to cancel."
                            .to_string(),
                    );
                    lines.join("\n")
                }
                _ => "Sequence approved — executing first step".to_string(),
            }
        };
        on_event(RuntimeEvent::SystemMessage(summary));
        self.handle_sequence_execute_step(on_event);
    }

    pub(super) fn handle_sequence_execute_step(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        use std::process::Stdio;

        use crate::runtime::patch::{apply_patch, PatchError};
        use crate::storage::tasks::{EditStep, SequenceStatus, StepStatus};

        let sequence_id = match self.active_sequence_id.clone() {
            Some(id) => id,
            None => {
                on_event(RuntimeEvent::Failed {
                    message: "No active sequence".to_string(),
                });
                return;
            }
        };

        loop {
            let store = match &self.edit_store {
                Some(s) => s,
                None => {
                    on_event(RuntimeEvent::Failed {
                        message: "No edit store configured".to_string(),
                    });
                    return;
                }
            };

            let step = match store.get_current_step(&sequence_id) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    // All steps done — sequence complete.
                    if let Some(store) = &self.edit_store {
                        let _ =
                            store.update_sequence_status(&sequence_id, SequenceStatus::Completed);
                    }
                    self.active_sequence_id = None;
                    on_event(RuntimeEvent::SystemMessage(
                        "Refactor sequence complete".to_string(),
                    ));
                    break;
                }
                Err(e) => {
                    on_event(RuntimeEvent::Failed {
                        message: format!("Failed to load step: {e}"),
                    });
                    return;
                }
            };

            // Placeholder-skip guard: generated steps with no edit body are skipped.
            if step.search.is_empty() && step.replace.is_empty() {
                if let Some(s) = &self.edit_store {
                    let _ = s.update_step_status(&step.id, StepStatus::Verified);
                    let _ = s.advance(&sequence_id);
                }
                on_event(RuntimeEvent::SystemMessage(format!(
                    "Step {} skipped — generated placeholder, no edit body",
                    step.position
                )));
                continue;
            }

            let original = match std::fs::read_to_string(&step.file) {
                Ok(s) => s,
                Err(e) => {
                    on_event(RuntimeEvent::Failed {
                        message: format!("Failed to read {}: {e}", step.file.display()),
                    });
                    self.handle_sequence_abort(on_event);
                    return;
                }
            };

            let mut before_syms: Vec<(String, String)> = Vec::new();
            let mut importers: Vec<String> = Vec::new();
            let seq_rel_path: Option<String> = step
                .file
                .strip_prefix(self.project_root.path())
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"));
            if let Some(store) = &self.symbol_store {
                let project_root = self.project_root.path().to_string_lossy().to_string();
                if let Some(ref rel_path) = seq_rel_path {
                    before_syms = store
                        .symbols_for_file(&project_root, rel_path)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|s| (s.name, s.signature))
                        .collect();
                    importers = store
                        .importers_of(&project_root, rel_path, 10)
                        .unwrap_or_default();
                    if !importers.is_empty() {
                        on_event(RuntimeEvent::SystemMessage(format!(
                            "affects: {}",
                            importers.join(", ")
                        )));
                    }
                }
            }

            let patch_text = format!(
                "<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE\n",
                step.search, step.replace
            );

            let patched = match apply_patch(&original, &patch_text) {
                Ok(p) => p,
                Err(e) => {
                    let detail = match &e {
                        PatchError::AnchorNotFound => "search text not found in file".to_string(),
                        PatchError::MultiplePatches => {
                            "expected exactly one patch block".to_string()
                        }
                        PatchError::ParseError(msg) => format!("parse error: {msg}"),
                    };
                    on_event(RuntimeEvent::Failed {
                        message: format!(
                            "Step {} patch failed ({}): {}",
                            step.position,
                            step.file.display(),
                            detail
                        ),
                    });
                    self.handle_sequence_abort(on_event);
                    return;
                }
            };

            if let Err(e) = std::fs::write(&step.file, &patched) {
                on_event(RuntimeEvent::Failed {
                    message: format!("Failed to write {}: {e}", step.file.display()),
                });
                self.handle_sequence_abort(on_event);
                return;
            }

            self.rebuild_index_for_file(&step.file, on_event);

            if let Some(verify_cmd) = self.verify_command.clone() {
                if let Some(error_output) = self.run_verify_command(&verify_cmd, on_event) {
                    // Restore current file before full transactional rollback.
                    let _ = std::fs::write(&step.file, &original);
                    on_event(RuntimeEvent::Failed {
                        message: format!(
                            "Verify failed for step {} — file restored\n{}",
                            step.position,
                            error_output.trim()
                        ),
                    });
                    self.handle_sequence_abort(on_event);
                    return;
                }
            }

            // Mark step verified and advance pointer.
            if let Some(store) = &self.edit_store {
                let _ = store.update_step_status(&step.id, StepStatus::Verified);
                let _ = store.advance(&sequence_id);
            }

            let diff_preview = crate::runtime::diff::render_diff(&original, &patched);
            // Populated below inside the signature-diff block; consumed by the live-insertion pass.
            let mut insertion_candidates: Vec<String> = Vec::new();
            let call_site_info: Option<String> = if let Some(ref rel_path) = seq_rel_path {
                if let Some(store) = &self.symbol_store {
                    use crate::storage::index::store::SymbolRecord;
                    let root = self.project_root.path().to_string_lossy().to_string();
                    let after_recs: Vec<SymbolRecord> =
                        store.symbols_for_file(&root, rel_path).unwrap_or_default();
                    let after_syms: Vec<(String, String)> = after_recs
                        .iter()
                        .map(|s| (s.name.clone(), s.signature.clone()))
                        .collect();
                    if let Some((diff_msg, changed_names)) =
                        diff_symbol_signatures(&before_syms, &after_syms)
                    {
                        let precise = if let Some(name) = changed_names.first() {
                            if let Some(sym) = after_recs.iter().find(|r| &r.name == name) {
                                if let Ok(locs) = self
                                    .lsp
                                    .query_references(&step.file, &patched, sym.line, sym.col)
                                {
                                    let project_root = self.project_root.path();
                                    let (norm_paths, display_lines): (Vec<String>, Vec<String>) =
                                        locs.into_iter()
                                            .filter_map(|loc| {
                                                loc.path.strip_prefix(project_root).ok().map(
                                                    |rel| {
                                                        (
                                                            rel.to_string_lossy()
                                                                .replace('\\', "/"),
                                                            format!(
                                                                "  {}:{}",
                                                                rel.display(),
                                                                loc.line
                                                            ),
                                                        )
                                                    },
                                                )
                                            })
                                            .unzip();
                                    insertion_candidates = norm_paths;
                                    if !display_lines.is_empty() {
                                        let total = display_lines.len();
                                        let capped: Vec<String> =
                                            display_lines.into_iter().take(10).collect();
                                        let header = if total > 10 {
                                            format!("{total} call sites (showing 10):")
                                        } else {
                                            format!("{total} call sites:")
                                        };
                                        Some(format!("{header}\n{}", capped.join("\n")))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let fallback = if !importers.is_empty() {
                            // Use importers as insertion candidates when LSP found nothing.
                            if insertion_candidates.is_empty() {
                                insertion_candidates =
                                    importers.iter().map(|f| f.replace('\\', "/")).collect();
                            }
                            let total = importers.len();
                            let capped: Vec<&String> = importers.iter().take(10).collect();
                            let header = if total > 10 {
                                format!("{total} affected files (showing 10):")
                            } else {
                                format!("{total} affected files:")
                            };
                            let list = capped
                                .into_iter()
                                .map(|f| format!("  {f}"))
                                .collect::<Vec<_>>()
                                .join("\n");
                            Some(format!("{diff_msg}\n{header}\n{list}"))
                        } else {
                            Some(diff_msg)
                        };
                        Some(precise.unwrap_or_else(|| fallback.unwrap_or_default()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // 43.8: Insert generated placeholder steps for call-site paths.
            if !insertion_candidates.is_empty() {
                let project_root = self.project_root.path();
                let remaining_files: std::collections::HashSet<String> =
                    if let Some(es) = &self.edit_store {
                        if let Ok(Some(seq)) = es.get_sequence(&sequence_id) {
                            seq.steps
                                .iter()
                                .filter(|s| s.position >= seq.current_idx)
                                .filter_map(|s| {
                                    s.file
                                        .strip_prefix(project_root)
                                        .ok()
                                        .map(|r| r.to_string_lossy().replace('\\', "/"))
                                })
                                .collect()
                        } else {
                            std::collections::HashSet::new()
                        }
                    } else {
                        std::collections::HashSet::new()
                    };

                let mut survivors: Vec<String> = insertion_candidates
                    .into_iter()
                    .filter(|p| !remaining_files.contains(p))
                    .collect();

                let live_total = if let Some(es) = &self.edit_store {
                    es.count_steps(&sequence_id).unwrap_or(0)
                } else {
                    0
                };

                const STEP_CAP: usize = 20;
                if live_total + survivors.len() > STEP_CAP {
                    let allowed = STEP_CAP.saturating_sub(live_total);
                    let dropped = survivors.len().saturating_sub(allowed);
                    survivors.truncate(allowed);
                    if dropped > 0 {
                        on_event(RuntimeEvent::SystemMessage(format!(
                            "step cap ({STEP_CAP}) reached — {dropped} call sites left unhandled; sequence will complete with current steps"
                        )));
                    }
                }

                if !survivors.is_empty() {
                    fn gen_step_id() -> String {
                        use std::sync::atomic::{AtomicU64, Ordering};
                        use std::time::{SystemTime, UNIX_EPOCH};
                        static COUNTER: AtomicU64 = AtomicU64::new(0);
                        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
                        let nanos = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(0);
                        let unique = nanos.wrapping_add(count) ^ (std::process::id() as u64);
                        format!("{unique:016x}")
                    }

                    let seq_id_clone = sequence_id.clone();
                    let new_steps: Vec<EditStep> = survivors
                        .iter()
                        .map(|path| EditStep {
                            id: gen_step_id(),
                            sequence_id: seq_id_clone.clone(),
                            position: 0, // overwritten by insert_step_after
                            file: project_root.join(path),
                            search: String::new(),
                            replace: String::new(),
                            verification_cmd: None,
                            status: StepStatus::Pending,
                            generated: true,
                        })
                        .collect();

                    if let Some(es) = &self.edit_store {
                        let _ = es.insert_step_after(&sequence_id, step.position, new_steps);
                    }
                }
            }
            let step_msg = if let Some(info) = call_site_info {
                format!(
                    "Step {} applied and verified: {}\n{}\n{}",
                    step.position,
                    step.file.display(),
                    diff_preview,
                    info
                )
            } else {
                format!(
                    "Step {} applied and verified: {}\n{}",
                    step.position,
                    step.file.display(),
                    diff_preview
                )
            };
            on_event(RuntimeEvent::SystemMessage(step_msg));

            // Best-effort git checkpoint — errors do not abort the sequence.
            let root = self.project_root.path().to_path_buf();
            let file_str = step.file.to_string_lossy().into_owned();
            let commit_msg = format!("thunk: apply step {}", step.position);
            let _ = std::process::Command::new("git")
                .args(["add", &file_str])
                .current_dir(&root)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            let _ = std::process::Command::new("git")
                .args(["commit", "-m", &commit_msg])
                .current_dir(&root)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
        }
    }

    pub(super) fn handle_sequence_abort(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        use std::process::Stdio;

        use crate::storage::tasks::SequenceStatus;

        let sequence_id = match self.active_sequence_id.take() {
            Some(id) => id,
            None => return,
        };

        if let Some(store) = &self.edit_store {
            let _ = store.update_sequence_status(&sequence_id, SequenceStatus::Failed);
        }

        // Attempt transactional rollback to the pre-sequence git snapshot.
        let snapshot = self
            .edit_store
            .as_ref()
            .and_then(|s| s.get_sequence(&sequence_id).ok().flatten())
            .and_then(|seq| seq.snapshot_ref);

        match snapshot {
            Some(sha) => {
                let root = self.project_root.path().to_path_buf();
                let reset_output = std::process::Command::new("git")
                    .args(["reset", "--hard", &sha])
                    .current_dir(&root)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output();

                match reset_output {
                    Ok(out) if out.status.success() => {
                        on_event(RuntimeEvent::SystemMessage(
                            "Refactor sequence aborted — all applied steps rolled back via git reset"
                                .to_string(),
                        ));
                    }
                    _ => {
                        on_event(RuntimeEvent::SystemMessage(
                            "Refactor sequence aborted — rollback failed; applied edits remain, check git status"
                                .to_string(),
                        ));
                    }
                }
            }
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "Refactor sequence aborted — no snapshot available; applied edits remain in place"
                        .to_string(),
                ));
            }
        }
    }

    pub(super) fn handle_sequence_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let sequence_id = match &self.active_sequence_id {
            Some(id) => id.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "No active refactor sequence".to_string(),
                ));
                return;
            }
        };

        let store = match &self.edit_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "No edit store configured".to_string(),
                ));
                return;
            }
        };

        match store.get_sequence(&sequence_id) {
            Ok(Some(seq)) => {
                use crate::storage::tasks::StepStatus;
                let total = seq.steps.len();
                let status = seq.status.as_str();
                let mut lines = vec![
                    format!("Refactor: {}", seq.goal),
                    format!("Step {}/{} — {}", seq.current_idx, total, status),
                ];
                for step in &seq.steps {
                    let indicator = if step.status == StepStatus::Verified {
                        '✓'
                    } else if step.position == seq.current_idx {
                        '→'
                    } else {
                        '·'
                    };
                    let badge = if step.generated { " [auto]" } else { "" };
                    lines.push(format!("  {} {}{}", indicator, step.file.display(), badge));
                }
                on_event(RuntimeEvent::SystemMessage(lines.join("\n")));
            }
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "Sequence not found".to_string(),
                ));
            }
            Err(e) => {
                on_event(RuntimeEvent::Failed {
                    message: format!("Failed to load sequence: {e}"),
                });
            }
        }
    }

    pub(super) fn handle_ability_toggle(
        &mut self,
        name: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match name.as_deref() {
            None | Some("status") => {
                let msg = match &self.active_ability {
                    Some(a) => format!("ability: {}", a.name),
                    None => "ability: none".into(),
                };
                on_event(RuntimeEvent::SystemMessage(msg));
            }
            Some("off") => {
                self.active_ability = None;
                self.prompt_physics.active_ability = None;
                trace_runtime_decision(on_event, "ability_cleared", &[]);
                on_event(RuntimeEvent::SystemMessage("ability: cleared".into()));
            }
            Some("list") => {
                let names = AbilityLoader::list_available(&self.thunk_dir);
                on_event(RuntimeEvent::SystemMessage(format!(
                    "abilities: {}",
                    names.join(", ")
                )));
            }
            Some(ability_name) => match AbilityLoader::load(ability_name, &self.thunk_dir) {
                Ok(content) => {
                    let name = content.name.clone();
                    self.active_ability = Some(content);
                    self.prompt_physics.active_ability = self.active_ability.clone();
                    trace_runtime_decision(
                        on_event,
                        "ability_activated",
                        &[("name", name.clone())],
                    );
                    on_event(RuntimeEvent::SystemMessage(format!("ability: {name}")));
                }
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!("ability error: {e}")));
                }
            },
        }
    }

    pub(super) fn handle_skill_toggle(
        &mut self,
        name: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match name.as_deref() {
            None | Some("status") => {
                let msg = match &self.active_skill {
                    Some(s) => format!("skill: {}", s.name),
                    None => "skill: none".into(),
                };
                on_event(RuntimeEvent::SystemMessage(msg));
            }
            Some("off") => {
                self.active_skill = None;
                self.prompt_physics.active_skill = None;
                trace_runtime_decision(on_event, "skill_cleared", &[]);
                on_event(RuntimeEvent::SystemMessage("skill: cleared".into()));
            }
            Some("list") => {
                let names = SkillLoader::list_available(&self.thunk_dir);
                on_event(RuntimeEvent::SystemMessage(format!(
                    "skills: {}",
                    names.join(", ")
                )));
            }
            Some(skill_name) => match SkillLoader::load(skill_name, &self.thunk_dir) {
                Ok(content) => {
                    let name = content.name.clone();
                    self.active_skill = Some(content);
                    self.prompt_physics.active_skill = self.active_skill.clone();
                    trace_runtime_decision(on_event, "skill_activated", &[("name", name.clone())]);
                    on_event(RuntimeEvent::SystemMessage(format!("skill: {name}")));
                }
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!("skill error: {e}")));
                }
            },
        }
    }

    pub(super) fn handle_fetch_url(&mut self, url: String, on_event: &mut dyn FnMut(RuntimeEvent)) {
        use crate::runtime::ResolvedToolInput;
        use crate::tools::core::WebFetchTool;
        use crate::tools::{Tool, ToolRunResult};

        let tool = WebFetchTool::new(self.web_fetch_enabled);
        let input = ResolvedToolInput::WebFetch { url };
        match tool.run(&input) {
            Ok(ToolRunResult::Immediate(output)) => {
                on_event(RuntimeEvent::InfoMessage(tool_codec::format_tool_result(
                    "web_fetch",
                    &output,
                )));
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!("fetch: {e}")));
            }
            _ => {}
        }
    }

    pub(super) fn handle_verify_mutation_toggle(
        &mut self,
        command: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match command {
            Some(ref s) if s == "off" => {
                self.verify_command = None;
                on_event(RuntimeEvent::SystemMessage("verify: disabled".to_string()));
            }
            Some(cmd) => {
                let msg = format!("verify: set to \"{}\"", cmd);
                self.verify_command = Some(cmd);
                on_event(RuntimeEvent::SystemMessage(msg));
            }
            None => {
                let status = match &self.verify_command {
                    Some(cmd) => format!("verify: \"{}\"", cmd),
                    None => "verify: disabled".to_string(),
                };
                on_event(RuntimeEvent::SystemMessage(status));
            }
        }
    }

    pub(super) fn handle_exec_toggle(
        &mut self,
        enabled: Option<bool>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match enabled {
            Some(true) => {
                self.exec_enabled = true;
                on_event(RuntimeEvent::SystemMessage(
                    "exec mode enabled — arbitrary shell commands will require approval"
                        .to_string(),
                ));
            }
            Some(false) => {
                self.exec_enabled = false;
                on_event(RuntimeEvent::SystemMessage(
                    "exec mode disabled".to_string(),
                ));
            }
            None => {
                let status = if self.exec_enabled {
                    "exec mode: enabled"
                } else {
                    "exec mode: disabled"
                };
                on_event(RuntimeEvent::SystemMessage(status.to_string()));
            }
        }
    }

    pub(super) fn handle_dnd_toggle(
        &mut self,
        enabled: Option<bool>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match enabled {
            Some(true) => {
                self.dnd_enabled = true;
                on_event(RuntimeEvent::SystemMessage(
                    "do not disturb enabled — proactive suggestions paused".to_string(),
                ));
            }
            Some(false) => {
                self.dnd_enabled = false;
                on_event(RuntimeEvent::SystemMessage(
                    "do not disturb disabled".to_string(),
                ));
            }
            None => {
                let status = if self.dnd_enabled {
                    "do not disturb: enabled"
                } else {
                    "do not disturb: disabled"
                };
                on_event(RuntimeEvent::SystemMessage(status.to_string()));
            }
        }
    }

    pub(super) fn handle_agent_run(
        &mut self,
        ability: String,
        target: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if !matches!(ability.as_str(), "review" | "investigate" | "refactor") {
            on_event(RuntimeEvent::SystemMessage(format!(
                "agent: unsupported ability '{ability}' — supported: review, investigate, refactor"
            )));
            return;
        }

        let ability_content = match AbilityLoader::load(&ability, &self.thunk_dir) {
            Ok(c) => c,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!("agent: {e}")));
                return;
            }
        };

        let saved_ability = self.active_ability.clone();
        let saved_pp_ability = self.prompt_physics.active_ability.clone();

        self.active_ability = Some(ability_content);
        self.prompt_physics.active_ability = self.active_ability.clone();

        let target_str = target.as_deref().unwrap_or("the current codebase");
        let anchor = target
            .as_deref()
            .map(|p| format!("Read {p} first.\n\n"))
            .unwrap_or_default();

        let augmented_prompt = match ability.as_str() {
            "review" => agent_prompts::build_review_prompt(target_str, &anchor),
            "investigate" => agent_prompts::build_investigate_prompt(target_str, &anchor),
            "refactor" => agent_prompts::build_refactor_prompt(target_str, &anchor),
            _ => unreachable!(),
        };

        // Seed a runtime-owned read_file call when the target is a file path.
        // NL anchor alone is insufficient — TurnContext cannot classify a mid-prompt
        // "Read X first" because classify_direct_read_mode only fires on leading verbs.
        if let Some(ref path) = target {
            if !path.ends_with('/') && looks_like_file_path(path) {
                if path.starts_with('/') || path.split('/').any(|c| c == "..") {
                    trace_runtime_decision(
                        on_event,
                        "agent_seed_skipped",
                        &[
                            ("reason", "path_escapes_root".into()),
                            ("path", path.clone()),
                        ],
                    );
                } else {
                    self.pending_runtime_call = Some(PendingRuntimeCall {
                        input: ToolInput::ReadFile { path: path.clone() },
                        seeded_pre_generation: true,
                    });
                }
            }
        }
        trace_runtime_decision(
            on_event,
            "agent_run_started",
            &[
                ("ability", ability.clone()),
                ("target", target_str.to_string()),
            ],
        );
        self.conversation.push_user(augmented_prompt);
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        self.run_turns(0, on_event);
        trace_runtime_decision(
            on_event,
            "agent_run_finished",
            &[("ability", ability.clone())],
        );

        if ability == "refactor" {
            if self.pending_action.is_some() {
                on_event(RuntimeEvent::SystemMessage(
                    "agent refactor: investigation triggered a mutation approval \
                     — resolve it first"
                        .into(),
                ));
            } else if self.task_store.is_none() {
                on_event(RuntimeEvent::SystemMessage(
                    "agent refactor: no storage configured".into(),
                ));
            } else {
                let root = self.project_root.path().to_string_lossy().to_string();
                let active_plan = self
                    .task_store
                    .as_ref()
                    .and_then(|s| s.get_active_plan(&self.session_id, &root).ok().flatten());
                if active_plan.is_some() {
                    on_event(RuntimeEvent::SystemMessage(
                        "agent refactor: an active plan already exists — use /plan abandon first"
                            .into(),
                    ));
                } else {
                    let plan_goal = format!("Refactor {target_str}");
                    on_event(RuntimeEvent::SystemMessage(
                        "agent refactor: generating plan...".into(),
                    ));
                    let mut parsed_steps = None;
                    for attempt in 0..2 {
                        if let Some(raw) = self.generate_plan_steps(&plan_goal) {
                            match parse_plan(&raw) {
                                Ok(steps) => {
                                    parsed_steps = Some(steps);
                                    break;
                                }
                                Err(_) if attempt == 0 => continue,
                                Err(_) => {}
                            }
                        }
                    }
                    match parsed_steps {
                        Some(steps) => {
                            let step_tuples: Vec<(String, String)> = steps
                                .iter()
                                .map(|s| (s.title.clone(), s.description.clone()))
                                .collect();
                            self.pending_plan = Some(PendingPlanDraft {
                                goal: plan_goal.clone(),
                                steps,
                            });
                            on_event(RuntimeEvent::PlanApprovalRequired {
                                goal: plan_goal,
                                steps: step_tuples,
                            });
                        }
                        None => {
                            on_event(RuntimeEvent::SystemMessage(
                                "agent refactor: could not generate a valid plan \
                                 — try again or use /plan directly"
                                    .into(),
                            ));
                        }
                    }
                }
            }
        }

        self.active_ability = saved_ability;
        self.prompt_physics.active_ability = saved_pp_ability;
    }

    pub(super) fn handle_depth_toggle(
        &mut self,
        depth: Option<InvestigationDepth>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match depth {
            Some(d) => {
                self.investigation_depth = d;
                trace_runtime_decision(on_event, "depth_toggled", &[("depth", d.as_str().into())]);
                on_event(RuntimeEvent::SystemMessage(format!(
                    "investigation depth: {}",
                    d.as_str()
                )));
            }
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "investigation depth: {}",
                    self.investigation_depth.as_str()
                )));
            }
        }
    }

    pub(super) fn handle_retrieval_log(
        &mut self,
        n: Option<usize>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let limit = n.unwrap_or(10);
        let Some(ref store) = self.retrieval_log_store else {
            on_event(RuntimeEvent::SystemMessage(
                "retrieval log: not available (no db path)".to_string(),
            ));
            return;
        };
        let project_root = self.project_root.path().to_string_lossy().to_string();
        let entries = match store.last_n(&project_root, limit) {
            Ok(e) => e,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "retrieval log: error reading log: {e}"
                )));
                return;
            }
        };
        if entries.is_empty() {
            on_event(RuntimeEvent::SystemMessage(
                "retrieval log: no entries yet".to_string(),
            ));
            return;
        }
        let mut lines = vec![format!("retrieval log (last {}):", entries.len())];
        for e in &entries {
            let vec_flag = if e.vector_augmented { " [vec]" } else { "" };
            let hops = if e.hops_taken > 0 {
                format!(" hops={}", e.hops_taken)
            } else {
                String::new()
            };
            lines.push(format!(
                "  strategy={} candidates={} reads={} gate={}{}{}",
                e.strategy,
                e.candidates_found,
                e.reads_accepted,
                e.evidence_outcome,
                hops,
                vec_flag,
            ));
        }
        on_event(RuntimeEvent::SystemMessage(lines.join("\n")));
    }
}

fn toposort_steps(
    steps: Vec<crate::storage::tasks::EditStep>,
    edges: &[crate::storage::index::types::ImportEdge],
    project_root: &std::path::Path,
) -> Vec<crate::storage::tasks::EditStep> {
    let n = steps.len();
    if n <= 1 {
        return steps;
    }

    let normalized: Vec<String> = steps
        .iter()
        .map(|s| {
            if let Ok(rel) = s.file.strip_prefix(project_root) {
                rel.to_string_lossy().replace('\\', "/")
            } else {
                s.file.to_string_lossy().replace('\\', "/")
            }
        })
        .collect();

    // adj[i] = steps that must come AFTER step i (i is a dependency of adj[i])
    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

    for edge in edges {
        let from_idx = normalized.iter().position(|p| p == &edge.from_file);
        let to_idx = normalized.iter().position(|p| p == &edge.to_file);
        if let (Some(from_i), Some(to_i)) = (from_idx, to_idx) {
            if from_i != to_i {
                // from_i imports to_i => to_i is a dependency; to_i must run before from_i
                adj[to_i].push(from_i);
                in_degree[from_i] += 1;
            }
        }
    }

    let mut result_indices: Vec<usize> = Vec::with_capacity(n);
    let mut available: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

    while !available.is_empty() {
        available.sort_unstable();
        let next = available.remove(0);
        result_indices.push(next);
        for &dep in &adj[next] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                available.push(dep);
            }
        }
    }

    if result_indices.len() < n {
        let in_result: std::collections::HashSet<usize> = result_indices.iter().copied().collect();
        let cycle_count = n - in_result.len();
        for i in 0..n {
            if !in_result.contains(&i) {
                result_indices.push(i);
            }
        }
        if std::env::var_os(crate::runtime::trace::RUNTIME_TRACE_ENV).is_some() {
            eprintln!("[runtime:trace] event=toposort_cycle steps_fell_back={cycle_count}");
        }
    }

    result_indices
        .into_iter()
        .enumerate()
        .map(|(new_pos, i)| {
            let mut step = steps[i].clone();
            step.position = new_pos;
            step
        })
        .collect()
}

pub(super) fn diff_symbol_signatures(
    before: &[(String, String)],
    after: &[(String, String)],
) -> Option<(String, Vec<String>)> {
    use std::collections::HashMap;
    let before_map: HashMap<&str, &str> = before
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    let after_map: HashMap<&str, &str> = after
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();

    let mut changed = Vec::new();
    let mut changed_names: Vec<String> = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();

    for (name, before_sig) in &before_map {
        match after_map.get(name) {
            Some(after_sig) if after_sig != before_sig => {
                changed.push(format!(
                    "  {name}\n    before: {before_sig}\n    after:  {after_sig}"
                ));
                changed_names.push(name.to_string());
            }
            None => removed.push(name.to_string()),
            _ => {}
        }
    }
    for name in after_map.keys() {
        if !before_map.contains_key(name) {
            added.push(name.to_string());
        }
    }

    if changed.is_empty() && added.is_empty() && removed.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    if !changed.is_empty() {
        changed.sort();
        parts.push(format!("Signature changed:\n{}", changed.join("\n")));
    }
    if !added.is_empty() {
        added.sort();
        parts.push(format!("Added: {}", added.join(", ")));
    }
    if !removed.is_empty() {
        removed.sort();
        parts.push(format!("Removed: {}", removed.join(", ")));
    }
    Some((parts.join("\n"), changed_names))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::Connection;

    use crate::runtime::types::RuntimeEvent;
    use crate::storage::tasks::{
        EditSequence, EditSequenceStore, EditStep, SequenceStatus, StepStatus,
    };

    fn make_test_store() -> EditSequenceStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS edit_sequences (
                id TEXT PRIMARY KEY, task_id TEXT, goal TEXT NOT NULL,
                current_idx INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'pending', snapshot_ref TEXT
             );
             CREATE TABLE IF NOT EXISTS edit_steps (
                id TEXT PRIMARY KEY, sequence_id TEXT NOT NULL,
                position INTEGER NOT NULL, file TEXT NOT NULL,
                search TEXT NOT NULL, replace TEXT NOT NULL,
                verification_cmd TEXT, status TEXT NOT NULL DEFAULT 'pending',
                generated INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();
        EditSequenceStore::new(conn)
    }

    fn make_test_sequence(id: &str, goal: &str, files: &[&str]) -> EditSequence {
        EditSequence {
            id: id.to_string(),
            task_id: None,
            goal: goal.to_string(),
            steps: files
                .iter()
                .enumerate()
                .map(|(i, f)| EditStep {
                    id: format!("{id}-step{i}"),
                    sequence_id: id.to_string(),
                    position: i,
                    file: PathBuf::from(f),
                    search: String::new(),
                    replace: String::new(),
                    verification_cmd: None,
                    status: StepStatus::Pending,
                    generated: false,
                })
                .collect(),
            current_idx: 0,
            status: SequenceStatus::Planning,
            snapshot_ref: None,
        }
    }

    #[test]
    fn handle_sequence_approve_summary_contains_step_count_and_files() {
        let mut runtime = crate::runtime::tests::make_runtime(vec![] as Vec<String>);
        let store = make_test_store();
        let seq = make_test_sequence("seq1", "test refactor", &["src/alpha.rs", "src/beta.rs"]);
        store.create_sequence(&seq).unwrap();
        runtime.edit_store = Some(store);
        runtime.active_sequence_id = Some("seq1".to_string());

        let mut events = Vec::new();
        runtime.handle_sequence_approve(&mut |e| events.push(e));

        let first_system_msg = events.iter().find_map(|e| {
            if let RuntimeEvent::SystemMessage(msg) = e {
                Some(msg.clone())
            } else {
                None
            }
        });
        let msg = first_system_msg.expect("expected a SystemMessage");
        assert!(msg.contains("2 steps"), "missing step count: {msg}");
        assert!(msg.contains("src/alpha.rs"), "missing first file: {msg}");
        assert!(msg.contains("src/beta.rs"), "missing second file: {msg}");
    }

    #[test]
    fn handle_sequence_status_shows_step_indicators() {
        let mut runtime = crate::runtime::tests::make_runtime(vec![] as Vec<String>);
        let store = make_test_store();
        let mut seq =
            make_test_sequence("seq2", "test goal", &["src/a.rs", "src/b.rs", "src/c.rs"]);
        seq.steps[0].status = StepStatus::Verified;
        seq.current_idx = 1;
        store.create_sequence(&seq).unwrap();
        runtime.edit_store = Some(store);
        runtime.active_sequence_id = Some("seq2".to_string());

        let mut events = Vec::new();
        runtime.handle_sequence_status(&mut |e| events.push(e));

        let system_msgs: Vec<String> = events
            .iter()
            .filter_map(|e| {
                if let RuntimeEvent::SystemMessage(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .collect();
        let combined = system_msgs.join("\n");
        assert!(
            combined.contains('✓'),
            "missing verified indicator: {combined}"
        );
        assert!(
            combined.contains('→'),
            "missing current indicator: {combined}"
        );
        assert!(
            combined.contains('·'),
            "missing pending indicator: {combined}"
        );
    }

    #[test]
    fn rebuild_index_for_file_no_op_when_store_is_none() {
        let mut runtime = crate::runtime::tests::make_runtime(vec![] as Vec<String>);
        // make_runtime has symbol_store = None by default.
        let mut events: Vec<RuntimeEvent> = Vec::new();
        runtime.rebuild_index_for_file(std::path::Path::new("/nonexistent/src/lib.rs"), &mut |e| {
            events.push(e)
        });
        let failed = events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::Failed { .. }));
        assert!(!failed, "rebuild must not emit Failed when store is None");
    }

    #[test]
    fn rebuild_index_for_file_no_op_when_store_is_empty() {
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let mut runtime = crate::runtime::tests::make_runtime(vec![] as Vec<String>)
            .with_symbol_store(tmp_db.path());
        let mut events: Vec<RuntimeEvent> = Vec::new();
        // Store is initialized but empty (no symbols), so is_empty() returns true → bail.
        runtime.rebuild_index_for_file(std::path::Path::new("/nonexistent/src/lib.rs"), &mut |e| {
            events.push(e)
        });
        let failed = events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::Failed { .. }));
        assert!(!failed, "rebuild must not emit Failed when store is empty");
        // No SystemMessage either — rebuild is entirely silent on the empty-store path.
        let system_msg = events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::SystemMessage(_)));
        assert!(
            !system_msg,
            "rebuild must not emit SystemMessage when store is empty"
        );
    }

    fn make_step(position: usize, file: &str) -> crate::storage::tasks::EditStep {
        crate::storage::tasks::EditStep {
            id: format!("step-{position}"),
            sequence_id: "seq".to_string(),
            position,
            file: PathBuf::from(file),
            search: String::new(),
            replace: String::new(),
            verification_cmd: None,
            status: crate::storage::tasks::StepStatus::Pending,
            generated: false,
        }
    }

    fn make_edge(from: &str, to: &str) -> crate::storage::index::types::ImportEdge {
        crate::storage::index::types::ImportEdge {
            from_file: from.to_string(),
            to_file: to.to_string(),
        }
    }

    #[test]
    fn toposort_steps_reorders_dependency_before_importer() {
        // A imports C => C must execute before A; B has no edges
        let steps = vec![
            make_step(0, "src/a.rs"),
            make_step(1, "src/b.rs"),
            make_step(2, "src/c.rs"),
        ];
        let edges = vec![make_edge("src/a.rs", "src/c.rs")];
        let root = PathBuf::from("/project");
        let result = super::toposort_steps(steps, &edges, &root);

        let files: Vec<&str> = result.iter().map(|s| s.file.to_str().unwrap()).collect();
        let pos_a = files.iter().position(|&f| f == "src/a.rs").unwrap();
        let pos_c = files.iter().position(|&f| f == "src/c.rs").unwrap();
        assert!(
            pos_c < pos_a,
            "c (dependency) must come before a (importer); order: {files:?}"
        );
        // positions updated contiguously
        for (i, step) in result.iter().enumerate() {
            assert_eq!(step.position, i, "position must equal new index");
        }
    }

    #[test]
    fn toposort_steps_no_edges_preserves_original_order() {
        let steps = vec![
            make_step(0, "src/a.rs"),
            make_step(1, "src/b.rs"),
            make_step(2, "src/c.rs"),
        ];
        let root = PathBuf::from("/project");
        let result = super::toposort_steps(steps, &[], &root);

        let files: Vec<&str> = result.iter().map(|s| s.file.to_str().unwrap()).collect();
        assert_eq!(
            files,
            vec!["src/a.rs", "src/b.rs", "src/c.rs"],
            "no edges must preserve original order"
        );
        for (i, step) in result.iter().enumerate() {
            assert_eq!(step.position, i);
        }
    }

    #[test]
    fn toposort_steps_cycle_falls_back_to_original_order() {
        // A imports B AND B imports A — cycle; both must fall back, no panic
        let steps = vec![make_step(0, "src/a.rs"), make_step(1, "src/b.rs")];
        let edges = vec![
            make_edge("src/a.rs", "src/b.rs"),
            make_edge("src/b.rs", "src/a.rs"),
        ];
        let root = PathBuf::from("/project");
        let result = super::toposort_steps(steps, &edges, &root);

        assert_eq!(
            result.len(),
            2,
            "all steps must be present after cycle fallback"
        );
        let files: Vec<&str> = result.iter().map(|s| s.file.to_str().unwrap()).collect();
        assert_eq!(
            files,
            vec!["src/a.rs", "src/b.rs"],
            "cycle must preserve original order"
        );
    }

    #[test]
    fn diff_symbol_signatures_detects_changed_added_removed() {
        let before = vec![
            ("foo".to_string(), "pub fn foo()".to_string()),
            ("bar".to_string(), "pub fn bar()".to_string()),
            ("gone".to_string(), "pub fn gone()".to_string()),
        ];
        let after = vec![
            ("foo".to_string(), "pub fn foo(x: u32)".to_string()),
            ("bar".to_string(), "pub fn bar()".to_string()),
            ("new_fn".to_string(), "pub fn new_fn()".to_string()),
        ];
        let (result, names) = super::diff_symbol_signatures(&before, &after).unwrap();
        assert!(result.contains("foo"), "changed symbol must appear");
        assert!(result.contains("pub fn foo()"), "before sig must appear");
        assert!(
            result.contains("pub fn foo(x: u32)"),
            "after sig must appear"
        );
        assert!(result.contains("new_fn"), "added symbol must appear");
        assert!(result.contains("gone"), "removed symbol must appear");
        assert!(!result.contains("bar"), "unchanged symbol must not appear");
        assert!(
            names.contains(&"foo".to_string()),
            "changed name must be tracked"
        );
        assert!(
            !names.contains(&"bar".to_string()),
            "unchanged must not be in names"
        );
    }

    #[test]
    fn diff_symbol_signatures_returns_none_when_unchanged() {
        let syms = vec![
            ("a".to_string(), "pub fn a()".to_string()),
            ("b".to_string(), "pub fn b()".to_string()),
        ];
        assert!(
            super::diff_symbol_signatures(&syms, &syms).is_none(),
            "identical sets must return None"
        );
    }

    #[test]
    fn diff_symbol_signatures_all_added_for_empty_before() {
        let after = vec![("new".to_string(), "pub fn new()".to_string())];
        let (result, names) = super::diff_symbol_signatures(&[], &after).unwrap();
        assert!(result.contains("new"));
        assert!(result.contains("Added"));
        assert!(names.is_empty(), "no changed names for all-added case");
    }

    #[test]
    fn handle_refactor_auto_generates_steps_for_signature_change_hint() {
        use crate::storage::index::types::ImportEdge;
        use crate::storage::index::SymbolStore;
        use crate::storage::session::schema;

        let root_dir = tempfile::TempDir::new().unwrap();
        let canon_root = root_dir.path().canonicalize().unwrap();
        let root_str = canon_root.to_string_lossy().into_owned();

        // Backend returns one step whose description contains "rename" → triggers importers_of
        let json =
            r#"[{"file":"src/lib.rs","description":"rename the process function signature"}]"#;

        // Seed symbol store: src/caller.rs imports src/lib.rs
        let sym_tmp = tempfile::NamedTempFile::new().unwrap();
        {
            let conn = Connection::open(sym_tmp.path()).unwrap();
            schema::initialize(&conn).unwrap();
            drop(conn);
            let store = SymbolStore::open(sym_tmp.path()).unwrap();
            store
                .upsert_imports(
                    &root_str,
                    &[ImportEdge {
                        from_file: "src/caller.rs".to_string(),
                        to_file: "src/lib.rs".to_string(),
                    }],
                )
                .unwrap();
        }

        // Seed edit store
        let edit_tmp = tempfile::NamedTempFile::new().unwrap();
        {
            let conn = Connection::open(edit_tmp.path()).unwrap();
            schema::initialize(&conn).unwrap();
        }

        let mut runtime = crate::runtime::tests::make_runtime_in(vec![json], &canon_root)
            .with_symbol_store(sym_tmp.path());
        runtime.edit_store = Some(EditSequenceStore::open(edit_tmp.path()).unwrap());

        let mut events = Vec::new();
        runtime.handle_refactor(None, &mut |e| events.push(e));

        let failed = events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::Failed { .. }));
        assert!(!failed, "handle_refactor should not fail: {events:?}");

        // Verify: one planned step + one auto-generated step persisted
        let conn = Connection::open(edit_tmp.path()).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM edit_steps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            total, 2,
            "expected 1 planned + 1 generated step, got {total}"
        );

        let gen_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edit_steps WHERE generated = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            gen_count, 1,
            "expected one generated step for the importer file"
        );

        let gen_file: String = conn
            .query_row("SELECT file FROM edit_steps WHERE generated = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            gen_file.contains("src/caller.rs"),
            "generated step should target the importer file, got: {gen_file}"
        );
    }
}
