use crate::llm::backend::Role;
use crate::tools::{ToolError, ToolInput, ToolRunResult};

use super::super::super::protocol::tool_codec;
use super::super::super::resolve;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{Activity, RuntimeEvent};
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
}

impl CommandTool {
    pub(super) fn into_input(self) -> ToolInput {
        match self {
            Self::ReadFile { path } => ToolInput::ReadFile { path },
            Self::SearchCode { query } => ToolInput::SearchCode { query, path: None },
            Self::GitBranch => ToolInput::GitBranch,
        }
    }

    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::ReadFile { .. } => "read_file",
            Self::SearchCode { .. } => "search_code",
            Self::GitBranch => "git_branch",
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
            CommandTool::ReadFile { .. } | CommandTool::GitBranch => None,
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
                self.pending_action = Some(pending.clone());
                on_event(RuntimeEvent::ApprovalRequired { pending, evidence: vec![] });
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
            let marker = if *internal == current { " (active)" } else { "" };
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
}
