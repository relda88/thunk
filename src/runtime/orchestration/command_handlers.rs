use crate::llm::backend::{BackendEvent, GenerateRequest, Message, Role};
use crate::storage::tasks::{PlanStatus, TaskStatus};
use crate::tools::{
    PendingApprovalStage, PendingTransaction, ToolError, ToolInput, ToolOutput, ToolRunResult,
};

use super::super::super::protocol::abilities::AbilityLoader;
use super::super::super::protocol::plan_parser::{parse_plan, PlanStep};
use super::super::super::protocol::skills::SkillLoader;
use super::super::super::protocol::tool_codec;
use super::super::super::resolve;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{Activity, RuntimeEvent};
use super::super::telemetry::TurnPerformance;
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
        self.dispatch_command_tool(CommandTool::SearchCode { query }, on_event);
    }

    pub(super) fn handle_reset(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.pending_action = None;
        self.pending_plan = None;
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
            let _ = store.upsert_imports(&project_root, &imports);
            // Record build timestamp via the project-level sentinel row.
            let _ = store.upsert_file_metadata(&project_root, "", now_secs, "");
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
            if count > 0 {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "context at {pct}% — auto-compacted {count} stale tool result(s)"
                )));
            }
            self.context_75_warned = true;
        } else if pct >= 75 && !self.context_75_warned {
            self.context_75_warned = true;
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
                    let _ = store.upsert_imports(&project_root, &imports);
                    let _ = store.upsert_file_metadata(&project_root, "", now_secs, "");
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
        let mut new_config = self.config.clone();
        new_config.llm.provider = normalized.to_string();
        match crate::llm::providers::build_backend(&new_config) {
            Ok(new_backend) => {
                self.backend = new_backend;
                self.config.llm.provider = normalized.to_string();
                on_event(RuntimeEvent::SystemMessage(format!(
                    "Switched to provider: {}",
                    normalized
                )));
            }
            Err(e) => {
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
                on_event(RuntimeEvent::SystemMessage(
                    "prompt physics: enabled".to_string(),
                ));
            }
            Some(false) => {
                self.prompt_physics.enabled = false;
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

    pub(super) fn handle_plan_create(
        &mut self,
        goal: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "plan: cannot create plan while a tool approval is pending".to_string(),
            ));
            return;
        }

        let project_root = self.project_root.path().to_string_lossy().to_string();
        if let Some(store) = &self.task_store {
            match store.get_active_plan(&self.session_id, &project_root) {
                Ok(Some(existing)) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "plan: '{}' is already active — use /plan abandon first",
                        existing.goal
                    )));
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "plan: storage error: {e}"
                    )));
                    return;
                }
            }
        }

        on_event(RuntimeEvent::SystemMessage(
            "plan: generating...".to_string(),
        ));

        let mut steps_result: Result<Vec<_>, String> = Err("no response".to_string());
        for attempt in 0..2 {
            match self.generate_plan_steps(&goal) {
                Some(text) => match parse_plan(&text) {
                    Ok(steps) => {
                        steps_result = Ok(steps);
                        break;
                    }
                    Err(e) => {
                        if attempt == 0 {
                            on_event(RuntimeEvent::SystemMessage(format!(
                                "plan: retrying... ({e})"
                            )));
                        } else {
                            steps_result = Err(e);
                        }
                    }
                },
                None => {
                    steps_result = Err("backend did not respond".to_string());
                    break;
                }
            }
        }

        match steps_result {
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "plan: could not parse model response — {e} — try again or simplify the goal"
                )));
            }
            Ok(steps) => {
                let step_pairs: Vec<(String, String)> = steps
                    .iter()
                    .map(|s| (s.title.clone(), s.description.clone()))
                    .collect();
                self.pending_plan = Some(PendingPlanDraft {
                    goal: goal.clone(),
                    steps,
                });
                on_event(RuntimeEvent::PlanApprovalRequired {
                    goal,
                    steps: step_pairs,
                });
            }
        }
    }

    pub(super) fn handle_plan_approve(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let draft = match self.pending_plan.take() {
            Some(d) => d,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "plan: no pending plan to approve".to_string(),
                ));
                return;
            }
        };

        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "plan: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan_id = match store.create_plan(&self.session_id, &project_root, &draft.goal) {
            Ok(id) => id,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "plan: failed to save plan: {e}"
                )));
                return;
            }
        };

        for (i, step) in draft.steps.iter().enumerate() {
            if let Err(e) = store.add_task(
                &plan_id,
                &self.session_id,
                &project_root,
                i + 1,
                &step.title,
                &step.description,
            ) {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "plan: failed to save step {}: {e}",
                    i + 1
                )));
                return;
            }
        }

        if let Err(e) = store.update_plan_status(&plan_id, PlanStatus::Active) {
            on_event(RuntimeEvent::SystemMessage(format!(
                "plan: failed to activate plan: {e}"
            )));
            return;
        }

        on_event(RuntimeEvent::PlanApprovalCleared);
        on_event(RuntimeEvent::SystemMessage(
            "plan: approved and saved — use /plan status to view steps".to_string(),
        ));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    pub(super) fn handle_plan_abandon(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.pending_plan = None;

        let project_root = self.project_root.path().to_string_lossy().to_string();
        if let Some(store) = &self.task_store {
            if let Ok(Some(plan)) = store.get_active_plan(&self.session_id, &project_root) {
                let _ = store.update_plan_status(&plan.id, PlanStatus::Abandoned);
            }
        }

        on_event(RuntimeEvent::PlanApprovalCleared);
        on_event(RuntimeEvent::SystemMessage("plan: abandoned".to_string()));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    pub(super) fn handle_plan_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if let Some(ref draft) = self.pending_plan {
            let mut msg = format!("plan (pending approval): {}\n", draft.goal);
            for (i, step) in draft.steps.iter().enumerate() {
                msg.push_str(&format!(
                    "  {}. {}: {}\n",
                    i + 1,
                    step.title,
                    step.description
                ));
            }
            msg.push_str("  Use /plan approve or /plan abandon");
            on_event(RuntimeEvent::SystemMessage(msg));
            return;
        }

        let project_root = self.project_root.path().to_string_lossy().to_string();
        if let Some(store) = &self.task_store {
            match store.get_active_plan(&self.session_id, &project_root) {
                Ok(Some(plan)) => {
                    let mut msg = format!("plan: {}\n", plan.goal);
                    match store.list_tasks(&plan.id) {
                        Ok(tasks) => {
                            for task in &tasks {
                                msg.push_str(&format!(
                                    "  {}. [{}] {}: {}\n",
                                    task.step_number,
                                    task.status.as_str(),
                                    task.title,
                                    task.description
                                ));
                            }
                        }
                        Err(e) => msg.push_str(&format!("  (error loading tasks: {e})")),
                    }
                    on_event(RuntimeEvent::SystemMessage(msg));
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "plan: storage error: {e}"
                    )));
                    return;
                }
            }
        }

        on_event(RuntimeEvent::SystemMessage(
            "plan: no active plan".to_string(),
        ));
    }

    fn generate_plan_steps(&mut self, goal: &str) -> Option<String> {
        let prompt = format!(
            "You are generating a structured implementation plan.\n\
             Respond with ONLY a numbered list — no preamble, no prose,\n\
             no markdown headers, no trailing commentary.\n\n\
             Format (one step per line):\n\
             1. Title: Brief description of this step\n\
             2. Title: Brief description of this step\n\n\
             Goal: {goal}\n\n\
             Requirements:\n\
             - At least 2 steps, at most 10 steps\n\
             - Each line must match exactly: N. Title: Description\n\
             - No blank lines between steps\n\
             - No text before or after the numbered list"
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
        let text = result.trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub(super) fn handle_task_execute(
        &mut self,
        step: usize,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "task: cannot execute while a tool approval is pending".to_string(),
            ));
            return;
        }

        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan = match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(p)) => p,
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan — use /plan first".to_string(),
                ));
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let tasks = match store.list_tasks(&plan.id) {
            Ok(t) => t,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let task = match tasks.iter().find(|t| t.step_number == step) {
            Some(t) => t.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: step {step} not found in active plan"
                )));
                return;
            }
        };

        let total = tasks.len();

        if task.status == TaskStatus::Completed {
            on_event(RuntimeEvent::SystemMessage(format!(
                "task: step {step} is already completed — executing anyway"
            )));
        }

        if let Some(store) = &self.task_store {
            let _ = store.update_task_status(&task.id, TaskStatus::InProgress, None);
        }

        let augmented_prompt = format!(
            "[runtime:task] Step {step} of {total}: \"{title}\"\n\
             Description: {description}\n\
             Plan goal: {goal}\n\n\
             Work on this step now.",
            title = task.title,
            description = task.description,
            goal = plan.goal,
        );

        self.conversation.push_user(augmented_prompt);
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        self.run_turns(0, on_event);
    }

    pub(super) fn handle_task_complete(
        &mut self,
        step: usize,
        summary: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan = match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(p)) => p,
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan".to_string(),
                ));
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let tasks = match store.list_tasks(&plan.id) {
            Ok(t) => t,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let task = match tasks.iter().find(|t| t.step_number == step) {
            Some(t) => t.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: step {step} not found in active plan"
                )));
                return;
            }
        };

        if let Err(e) =
            store.update_task_status(&task.id, TaskStatus::Completed, summary.as_deref())
        {
            on_event(RuntimeEvent::SystemMessage(format!(
                "task: storage error: {e}"
            )));
            return;
        }

        on_event(RuntimeEvent::SystemMessage(format!(
            "task {step}: completed"
        )));

        let all_done = tasks
            .iter()
            .filter(|t| t.step_number != step)
            .all(|t| t.status == TaskStatus::Completed);
        if all_done {
            if let Some(store) = &self.task_store {
                let _ = store.update_plan_status(&plan.id, PlanStatus::Completed);
            }
            on_event(RuntimeEvent::SystemMessage(
                "plan: all steps completed".to_string(),
            ));
        }
    }

    pub(super) fn handle_task_block(
        &mut self,
        step: usize,
        reason: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan = match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(p)) => p,
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan".to_string(),
                ));
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let tasks = match store.list_tasks(&plan.id) {
            Ok(t) => t,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let task = match tasks.iter().find(|t| t.step_number == step) {
            Some(t) => t.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: step {step} not found in active plan"
                )));
                return;
            }
        };

        if let Err(e) = store.update_task_status(&task.id, TaskStatus::Blocked, reason.as_deref()) {
            on_event(RuntimeEvent::SystemMessage(format!(
                "task: storage error: {e}"
            )));
            return;
        }

        on_event(RuntimeEvent::SystemMessage(format!("task {step}: blocked")));
    }

    pub(super) fn handle_task_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(plan)) => {
                let mut msg = format!("plan: {}\n", plan.goal);
                match store.list_tasks(&plan.id) {
                    Ok(tasks) => {
                        for task in &tasks {
                            let badge = match task.status {
                                TaskStatus::Pending => "[pending]",
                                TaskStatus::InProgress => "[in_progress]",
                                TaskStatus::Completed => "[completed]",
                                TaskStatus::Blocked => "[blocked]",
                            };
                            msg.push_str(&format!(
                                "  {}. {} {}: {}\n",
                                task.step_number, badge, task.title, task.description
                            ));
                        }
                    }
                    Err(e) => msg.push_str(&format!("  (error loading tasks: {e})")),
                }
                on_event(RuntimeEvent::SystemMessage(msg));
            }
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan".to_string(),
                ));
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
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
            "review" => format!(
                "[runtime:agent:review] Review workflow\n\
                 Target: {target_str}\n\n\
                 {anchor}\
                 Use the review ability. Read the relevant files. \
                 Produce a structured critique.\n\n\
                 Format each issue as:\n\
                 [critical|warning|note] <what is wrong> — \
                 <why it matters> — <fix direction>\n\n\
                 Correctness bugs first. Cite specific file and \
                 line for every claim. Do not soften real bugs."
            ),
            "investigate" => format!(
                "[runtime:agent:investigate] Investigate workflow\n\
                 Target: {target_str}\n\n\
                 {anchor}\
                 Use the investigate ability. Gather evidence \
                 systematically.\n\n\
                 Format your findings as:\n\
                 KNOWN: <confirmed facts with evidence>\n\
                 UNKNOWN: <open questions, ranked by value>\n\
                 HYPOTHESIS: <current best explanation>\n\
                 NEXT READS: <specific files/symbols to resolve \
                 the top unknown>\n\n\
                 Do not conclude until evidence is sufficient."
            ),
            "refactor" => format!(
                "[runtime:agent:refactor] Refactor investigation\n\
                 Target: {target_str}\n\n\
                 {anchor}\
                 Use the refactor ability. Investigate the target. \
                 Identify structural problems: coupling, duplication, \
                 unclear naming, god functions, mixed responsibilities.\n\n\
                 Report what you found — structure, problems, and \
                 what a clean refactor would address. Be specific."
            ),
            _ => unreachable!(),
        };

        self.conversation.push_user(augmented_prompt);
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        self.run_turns(0, on_event);

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
}
