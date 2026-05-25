use crate::app::config::Config;
use crate::app::paths::AppPaths;

/// Defines the application state, including the current input, cursor position, message history, and status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

/// Represents a chat message with a role (system, user, assistant) and content
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
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
            welcome_message: welcome,
        }
    }

    /// Adds a system message to the transcript
    pub fn add_system_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::System,
            content: content.into(),
        });
        self.reset_scroll();
    }

    /// Adds a user message to the transcript
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::User,
            content: content.into(),
        });
        self.reset_scroll();
    }

    /// Adds a complete assistant message to the transcript
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::Assistant,
            content: content.into(),
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
            }) => content.push_str(chunk),
            _ => self.add_assistant_message(chunk.to_string()),
        }
    }

    /// Adds a tool-related notification to the transcript (shown as a system message).
    pub fn add_tool_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role: Role::System,
            content: content.into(),
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
        });
        self.reset_scroll();
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(self.max_scroll);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// Updates the visible status line
    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
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
        Some(submitted)
    }

    pub fn toggle_file_expand(&mut self) {
        self.expanded_file_read = !self.expanded_file_read;
    }

    pub fn store_file_read(&mut self, message_index: usize) {
        self.last_file_read_index = Some(message_index);
        self.expanded_file_read = false;
    }
}
