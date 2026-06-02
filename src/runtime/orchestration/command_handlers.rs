use crate::llm::backend::Role;
use crate::tools::{PendingApprovalStage, ToolError, ToolInput, ToolRunResult};

use super::super::super::protocol::tool_codec;
use super::super::super::resolve;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{Activity, RuntimeEvent};
use super::super::telemetry::TurnPerformance;
use super::Runtime;

/// Bounds for /history output. Limits messages shown and chars per message to
/// prevent unbounded InfoMessage output from long or tool-heavy sessions.
const MAX_HISTORY_MESSAGES: usize = 10;
const MAX_MESSAGE_CHARS: usize = 200;

/// Explicit allowlist of tools that slash commands may invoke via the runtime.
/// All command-to-registry dispatch passes through this type — no command handler
/// calls registry.dispatch() directly or constructs ToolInput outside this enum.
/// Mutating tools are excluded by omission; adding one requires an explicit variant.
pub(super) enum CommandTool {
    ReadFile { path: String },
    SearchCode { query: String },
    GitBranch,
    GitStatus,
    GitDiff,
    GitLog,
    ListDir { path: String },
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
                self.pending_action = Some(PendingApprovalStage::AwaitingPreCheck(pending.clone()));
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

    pub(super) fn handle_verify_mutation_toggle(
        &mut self,
        enabled: Option<bool>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        match enabled {
            Some(true) => {
                self.verify_after_mutation = true;
                on_event(RuntimeEvent::SystemMessage(
                    "verify after mutation: enabled".to_string(),
                ));
            }
            Some(false) => {
                self.verify_after_mutation = false;
                on_event(RuntimeEvent::SystemMessage(
                    "verify after mutation: disabled".to_string(),
                ));
            }
            None => {
                let status = if self.verify_after_mutation {
                    "verify after mutation: enabled"
                } else {
                    "verify after mutation: disabled"
                };
                on_event(RuntimeEvent::SystemMessage(status.to_string()));
            }
        }
    }
}
