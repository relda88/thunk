use std::collections::HashSet;

use crate::app::paths::AppPaths;
use crate::core::config::Config;

/// Defines the application state, including the current input, cursor position, message history, and status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Normal,
    Dimmed,
    Alert,
    Error,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct DirtySections(u8);

impl DirtySections {
    pub(crate) const HEADER: Self = Self(0b0001);
    pub(crate) const TRANSCRIPT: Self = Self(0b0010);
    pub(crate) const INPUT: Self = Self(0b0100);
    pub(crate) const STATUS: Self = Self(0b1000);
    pub(crate) const ALL: Self = Self(0b1111);
}

impl std::ops::BitOrAssign for DirtySections {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalRisk {
    Low,
    Medium,
    High,
}

pub(crate) struct PendingApprovalState {
    pub(crate) tool_name: String,
    pub(crate) summary: String,
    pub(crate) risk: ApprovalRisk,
    pub(crate) evidence: Vec<String>,
    pub(crate) preview: Vec<String>,
}

/// Represents a chat message with a role (system, user, assistant) and content
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub kind: MessageKind,
    pub is_collapsible: bool,
}

/// Main application state struct, holding the app name, input buffer, cursor position, message history, status, and quit flag
pub struct AppState {
    pub app_name: String,
    pub show_activity: bool,
    pub input: String,
    pub cursor: usize,
    pub messages: Vec<ChatMessage>,
    pub status: String,
    pub should_quit: bool,
    pub last_prompt: Option<String>,
    pub scroll_offset: usize,
    pub max_scroll: usize,
    pub expanded_file_read: bool,
    pub last_file_read_index: Option<usize>,
    /// Approximate context window usage (0–100). None when context window size is unknown.
    pub context_pct: Option<u8>,
    pub(crate) dirty_sections: DirtySections,
    /// True while a WorkerCmd is in flight and we're waiting for the terminal WorkerReply.
    pub(crate) is_busy: bool,
    pub(crate) input_history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) history_draft: Option<String>,
    pub(crate) reverse_search_active: bool,
    pub(crate) reverse_search_query: String,
    pub(crate) reverse_search_selection: usize,
    pub(crate) reverse_search_draft: Option<String>,
    pub(crate) collapsed_message_indices: HashSet<usize>,
    pub(crate) collapsible_message_indices: Vec<usize>,
    pub(crate) focused_collapsible_idx: Option<usize>,
    /// Set by focus_next/prev_collapsible; consumed by the renderer to scroll
    /// the newly focused message into the upper third of the viewport.
    pub(crate) scroll_to_message_idx: Option<usize>,
    pub(crate) pending_approval: Option<PendingApprovalState>,
    pub(crate) autocomplete_matches: Vec<String>,
    pub(crate) autocomplete_index: usize,
    pub(crate) autocomplete_prefix: Option<String>,
    // Stored once at construction; used to restore messages on /clear.
    welcome_message: String,
}

/// Defines methods for modifying the input buffer and cursor position in the app state
impl AppState {
    /// Creates a new AppState instance, initializing the message history with a system message based on the provided config and paths
    pub fn new(config: &Config, paths: &AppPaths) -> Self {
        let welcome = format!(
            "{} ready. Root: {}. Press Ctrl+Q to quit.",
            config.app.name,
            paths.root_dir.display()
        );
        let messages = vec![ChatMessage {
            role: Role::System,
            content: welcome.clone(),
            kind: MessageKind::Normal,
            is_collapsible: false,
        }];

        Self {
            app_name: config.app.name.clone(),
            show_activity: config.ui.show_activity,
            input: String::new(),
            cursor: 0,
            messages,
            status: "ready".to_string(),
            should_quit: false,
            last_prompt: None,
            scroll_offset: 0,
            max_scroll: 0,
            expanded_file_read: false,
            last_file_read_index: None,
            context_pct: None,
            dirty_sections: DirtySections::ALL,
            is_busy: false,
            input_history: Vec::new(),
            history_cursor: None,
            history_draft: None,
            reverse_search_active: false,
            reverse_search_query: String::new(),
            reverse_search_selection: 0,
            reverse_search_draft: None,
            collapsed_message_indices: HashSet::new(),
            collapsible_message_indices: Vec::new(),
            focused_collapsible_idx: None,
            scroll_to_message_idx: None,
            pending_approval: None,
            autocomplete_matches: Vec::new(),
            autocomplete_index: 0,
            autocomplete_prefix: None,
            welcome_message: welcome,
        }
    }

    /// Adds a system message to the transcript
    pub fn add_system_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::System,
            content: content.into(),
            kind: MessageKind::Dimmed,
            is_collapsible: false,
        });
        self.reset_scroll();
    }

    /// Adds a user message to the transcript
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::User,
            content: content.into(),
            kind: MessageKind::Normal,
            is_collapsible: false,
        });
        self.reset_scroll();
    }

    /// Adds a complete assistant message to the transcript
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::Assistant,
            content: content.into(),
            kind: MessageKind::Normal,
            is_collapsible: false,
        });
        self.reset_scroll();
    }

    /// Starts a new assistant message so chunks can be streamed into it
    pub fn begin_assistant_message(&mut self) {
        self.add_assistant_message(String::new());
    }

    /// Appends text to the active assistant message, creating one if needed
    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        match self.messages.last_mut() {
            Some(ChatMessage {
                role: Role::Assistant,
                content,
                ..
            }) => content.push_str(chunk),
            _ => self.add_assistant_message(chunk.to_string()),
        }
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    /// Adds a tool-related notification to the transcript (shown as a system message).
    pub fn add_tool_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::System,
            content: content.into(),
            kind: MessageKind::Dimmed,
            is_collapsible: false,
        });
        self.reset_scroll();
    }

    /// Adds a collapsible tool-related notification to the transcript.
    pub fn add_collapsible_tool_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::System,
            content: content.into(),
            kind: MessageKind::Dimmed,
            is_collapsible: true,
        });
        self.reset_scroll();
        self.tag_last_message_collapsible();
    }

    pub fn add_error_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::System,
            content: content.into(),
            kind: MessageKind::Error,
            is_collapsible: false,
        });
        self.reset_scroll();
    }

    /// Clears all transcript messages and restores only the initial welcome line.
    /// Does not affect the runtime conversation — call RuntimeRequest::Reset separately.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.messages.push(ChatMessage {
            role: Role::System,
            content: self.welcome_message.clone(),
            kind: MessageKind::Normal,
            is_collapsible: false,
        });
        self.collapsed_message_indices.clear();
        self.collapsible_message_indices.clear();
        self.focused_collapsible_idx = None;
        self.scroll_to_message_idx = None;
        self.pending_approval = None;
        self.reset_scroll();
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(self.max_scroll);
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    /// Updates the visible status line
    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
        self.mark_dirty(DirtySections::STATUS);
    }

    pub fn set_last_prompt(&mut self, prompt: String) {
        self.last_prompt = Some(prompt);
    }

    /// Submits the current input, returning it as a string if it's not empty, and clears the input buffer and resets the cursor position
    pub fn submit_input(&mut self) -> Option<String> {
        if self.input.trim().is_empty() {
            self.clear_input();
            return None;
        }

        let submitted = std::mem::take(&mut self.input);
        self.cursor = 0;
        if !submitted.starts_with('/') {
            self.input_history.push(submitted.clone());
        }
        self.exit_reverse_search();
        self.clear_autocomplete();
        self.mark_dirty(DirtySections::INPUT);
        Some(submitted)
    }

    pub fn toggle_file_expand(&mut self) {
        self.expanded_file_read = !self.expanded_file_read;
        self.scroll_offset = 0;
        self.scroll_to_message_idx = None;
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    pub fn store_file_read(&mut self, message_index: usize) {
        self.last_file_read_index = Some(message_index);
        self.expanded_file_read = false;
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    /// If the last message is collapsible, records its index in collapsible_message_indices.
    pub(crate) fn tag_last_message_collapsible(&mut self) {
        let idx = self.messages.len().saturating_sub(1);
        if self.messages.get(idx).map_or(false, |m| m.is_collapsible) {
            self.collapsible_message_indices.push(idx);
        }
    }

    /// Toggles collapsed state on the focused collapsible message.
    pub(crate) fn toggle_collapse_focused(&mut self) {
        let Some(list_pos) = self.focused_collapsible_idx else {
            return;
        };
        let Some(&msg_idx) = self.collapsible_message_indices.get(list_pos) else {
            return;
        };
        if self.collapsed_message_indices.contains(&msg_idx) {
            self.collapsed_message_indices.remove(&msg_idx);
        } else {
            self.collapsed_message_indices.insert(msg_idx);
        }
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    /// Advances focus to the next collapsible message (wraps around).
    pub(crate) fn focus_next_collapsible(&mut self) {
        if self.collapsible_message_indices.is_empty() {
            return;
        }
        let new_pos = match self.focused_collapsible_idx {
            None => 0,
            Some(i) => (i + 1) % self.collapsible_message_indices.len(),
        };
        self.focused_collapsible_idx = Some(new_pos);
        self.scroll_to_message_idx = Some(self.collapsible_message_indices[new_pos]);
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    /// Retreats focus to the previous collapsible message (wraps around).
    pub(crate) fn focus_prev_collapsible(&mut self) {
        if self.collapsible_message_indices.is_empty() {
            return;
        }
        let new_pos = match self.focused_collapsible_idx {
            None => self.collapsible_message_indices.len() - 1,
            Some(0) => self.collapsible_message_indices.len() - 1,
            Some(i) => i - 1,
        };
        self.focused_collapsible_idx = Some(new_pos);
        self.scroll_to_message_idx = Some(self.collapsible_message_indices[new_pos]);
        self.mark_dirty(DirtySections::TRANSCRIPT);
    }

    pub(crate) fn mark_dirty(&mut self, s: DirtySections) {
        self.dirty_sections |= s;
    }

    pub(crate) fn has_dirty_sections(&self) -> bool {
        self.dirty_sections.0 != 0
    }

    pub(crate) fn clear_dirty_sections(&mut self) {
        self.dirty_sections = DirtySections(0);
    }

    pub(crate) fn set_context_pct(&mut self, pct: u8) {
        self.context_pct = Some(pct);
        self.mark_dirty(DirtySections::STATUS);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::app::paths::AppPaths;
    use crate::core::config::Config;

    use super::AppState;

    fn make_state() -> AppState {
        let config = Config::default();
        let paths = AppPaths {
            root_dir: PathBuf::from("/tmp"),
            project_root: PathBuf::from("/tmp"),
            config_file: PathBuf::from("/tmp/config.toml"),
            data_dir: PathBuf::from("/tmp/data"),
            logs_dir: PathBuf::from("/tmp/logs"),
            session_db: PathBuf::from("/tmp/data/sessions.db"),
        };
        AppState::new(&config, &paths)
    }

    #[test]
    fn toggle_collapse_focused_with_no_focus_does_nothing() {
        let mut state = make_state();
        state.add_collapsible_tool_message("tool output");
        assert!(state.focused_collapsible_idx.is_none());
        state.toggle_collapse_focused();
        assert!(
            state.collapsed_message_indices.is_empty(),
            "no collapse when no focus"
        );
    }

    #[test]
    fn focus_next_collapsible_cycles_correctly() {
        let mut state = make_state();
        state.add_collapsible_tool_message("a");
        state.add_collapsible_tool_message("b");
        state.add_collapsible_tool_message("c");
        assert_eq!(state.collapsible_message_indices.len(), 3);

        state.focus_next_collapsible();
        assert_eq!(state.focused_collapsible_idx, Some(0));

        state.focus_next_collapsible();
        assert_eq!(state.focused_collapsible_idx, Some(1));

        state.focus_next_collapsible();
        assert_eq!(state.focused_collapsible_idx, Some(2));

        // Wraps back to 0.
        state.focus_next_collapsible();
        assert_eq!(state.focused_collapsible_idx, Some(0));
    }

    #[test]
    fn focus_prev_collapsible_cycles_correctly() {
        let mut state = make_state();
        state.add_collapsible_tool_message("a");
        state.add_collapsible_tool_message("b");
        assert_eq!(state.collapsible_message_indices.len(), 2);

        state.focus_prev_collapsible();
        // Starting from None, wraps to last index.
        assert_eq!(state.focused_collapsible_idx, Some(1));

        state.focus_prev_collapsible();
        assert_eq!(state.focused_collapsible_idx, Some(0));

        // Wraps back to last.
        state.focus_prev_collapsible();
        assert_eq!(state.focused_collapsible_idx, Some(1));
    }

    #[test]
    fn clear_messages_resets_collapse_state() {
        let mut state = make_state();
        state.add_collapsible_tool_message("tool output");
        state.focus_next_collapsible();
        state.toggle_collapse_focused();
        assert!(!state.collapsed_message_indices.is_empty());
        assert!(!state.collapsible_message_indices.is_empty());
        assert!(state.focused_collapsible_idx.is_some());

        state.clear_messages();

        assert!(
            state.collapsed_message_indices.is_empty(),
            "collapse set must reset"
        );
        assert!(
            state.collapsible_message_indices.is_empty(),
            "collapsible list must reset"
        );
        assert!(state.focused_collapsible_idx.is_none(), "focus must reset");
    }

    #[test]
    fn tag_last_message_collapsible_does_not_tag_non_collapsible() {
        let mut state = make_state();
        state.add_system_message("system info");
        state.add_user_message("user prompt");
        // These calls do NOT go through add_collapsible_tool_message, so tag is never called.
        assert!(
            state.collapsible_message_indices.is_empty(),
            "non-collapsible messages must not be tagged"
        );
    }

    #[test]
    fn toggle_collapse_focused_collapses_then_expands() {
        let mut state = make_state();
        state.add_collapsible_tool_message("tool output");
        state.focus_next_collapsible();
        let msg_idx = state.collapsible_message_indices[0];

        state.toggle_collapse_focused();
        assert!(
            state.collapsed_message_indices.contains(&msg_idx),
            "should be collapsed"
        );

        state.toggle_collapse_focused();
        assert!(
            !state.collapsed_message_indices.contains(&msg_idx),
            "should be expanded again"
        );
    }

    #[test]
    fn clear_messages_resets_pending_approval() {
        let mut state = make_state();
        state.pending_approval = Some(super::PendingApprovalState {
            tool_name: "shell".into(),
            summary: "run tests".into(),
            risk: super::ApprovalRisk::High,
            evidence: vec![],
            preview: vec![],
        });
        assert!(state.pending_approval.is_some());

        state.clear_messages();
        assert!(
            state.pending_approval.is_none(),
            "clear_messages must reset pending_approval"
        );
    }
}
