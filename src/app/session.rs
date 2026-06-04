use std::path::{Path, PathBuf};

use crate::llm::backend::{Message, Role};
use crate::runtime::ProjectRoot;
use crate::storage::session::{SavedSession, SessionId, SessionMeta, SessionStore, StoredMessage};

use super::Result;

/// Owns the active database handle and current session ID.
/// Responsible for: opening/creating sessions, auto-saving, and the
/// explicit conversion between runtime Message types and stored records.
/// This is the only layer permitted to see both types simultaneously.
pub struct ActiveSession {
    store: SessionStore,
    session_id: SessionId,
    project_root: PathBuf,
}

impl ActiveSession {
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Opens the session database and returns the active session, previously stored messages,
    /// and restored anchor state. Returns empty messages and None anchors if no prior session exists.
    pub fn open_or_restore(
        db_path: &Path,
        project_root: &ProjectRoot,
    ) -> Result<(
        Self,
        Vec<Message>,
        (Option<String>, Option<String>, Option<String>),
    )> {
        let store = SessionStore::open(db_path)?;
        let current_root = project_root.path();
        let current_root_str = current_root.to_string_lossy();

        match store.load_most_recent_for_project(current_root_str.as_ref())? {
            Some(saved) => {
                let messages = from_stored(&saved);
                let anchors = (
                    saved.meta.last_read_file.clone(),
                    saved.meta.last_search_query.clone(),
                    saved.meta.last_search_scope.clone(),
                );
                let session_id = saved.meta.id;
                Ok((
                    Self {
                        store,
                        session_id,
                        project_root: current_root.to_path_buf(),
                    },
                    messages,
                    anchors,
                ))
            }
            None => {
                let meta = store.create(current_root)?;
                Ok((
                    Self {
                        store,
                        session_id: meta.id,
                        project_root: current_root.to_path_buf(),
                    },
                    vec![],
                    (None, None, None),
                ))
            }
        }
    }

    /// Persists the current conversation state and anchor fields.
    /// The caller provides the full runtime message list; system messages are stripped before storage.
    pub fn save(
        &self,
        runtime_messages: &[Message],
        anchors: (Option<String>, Option<String>, Option<String>),
    ) -> Result<()> {
        let stored = to_stored(runtime_messages);
        let (lrf, lsq, lss) = anchors;
        self.store.save(
            &self.session_id,
            &stored,
            lrf.as_deref(),
            lsq.as_deref(),
            lss.as_deref(),
        )?;
        Ok(())
    }

    /// Creates a new session and makes it the active one.
    /// Called when the user explicitly starts a fresh conversation.
    pub fn begin_new(&mut self) -> Result<()> {
        let meta = self.store.create(&self.project_root)?;
        self.session_id = meta.id;
        Ok(())
    }

    /// Returns metadata for all sessions belonging to the current project, newest first.
    pub fn list_for_project(&self) -> Result<Vec<SessionMeta>> {
        let root = self.project_root.to_string_lossy();
        self.store.list_for_project(root.as_ref())
    }

    /// Deletes all sessions for the current project and starts a fresh one.
    pub fn clear_for_project(&mut self) -> Result<()> {
        let root = self.project_root.to_string_lossy().into_owned();
        self.store.delete_for_project(&root)?;
        let meta = self.store.create(&self.project_root)?;
        self.session_id = meta.id;
        Ok(())
    }
}

// Conversion: runtime <--> storage
//
// System messages are excluded. The system prompt is reconstructed at runtime
// from config; storing it would create a stale copy that could diverge.

/// Maximum number of messages to inject into a fresh conversation on restore.
/// Prevents large accumulated histories from overflowing the model's context window.
const RESTORE_WINDOW: usize = 40;
const SUMMARY_GOAL_CAP: usize = 4;
const SUMMARY_DECISION_CAP: usize = 4;
const SUMMARY_FILE_CAP: usize = 8;
const SUMMARY_SEARCH_CAP: usize = 6;
const SUMMARY_ITEM_MAX_CHARS: usize = 120;

/// Converts runtime messages to storable form, excluding system messages.
fn to_stored(messages: &[Message]) -> Vec<StoredMessage> {
    messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| StoredMessage {
            role: m.role.as_str().to_string(),
            content: m.content.clone(),
        })
        .collect()
}

/// Converts stored messages back to runtime form, applying two rules:
///
/// 1. Window trim — only the most recent RESTORE_WINDOW messages are loaded.
///    Older history stays in the DB but is not injected into context.
///
/// 2. Tool exchange stripping — user messages that are runtime tool results or errors
///    are dropped entirely, along with the immediately preceding assistant message if
///    it was a pure tool call (starts with `[`). Raw file contents and directory
///    listings are never re-injected into the context window on restore. Full content
///    is preserved in storage; only context injection is affected.
///
///    Placeholders are intentionally not used: a placeholder that looks like a real
///    tool result causes the model to believe the exchange already completed, suppressing
///    fresh tool use when the user re-requests the same operation.
fn from_stored(session: &SavedSession) -> Vec<Message> {
    let total = session.messages.len();
    let exclude = build_restore_exclusions(&session.messages);
    let start = total.saturating_sub(RESTORE_WINDOW);
    let mut restored = Vec::new();

    if total > RESTORE_WINDOW {
        let summary = build_restore_summary(&session.messages[..start], &exclude[..start]);
        restored.push(Message::system(summary));
    }

    restored.extend(
        session.messages[start..]
            .iter()
            .zip(exclude[start..].iter())
            .filter(|(_, &ex)| !ex)
            .filter_map(|(m, _)| match m.role.as_str() {
                "user" => Some(Message::user(m.content.clone())),
                "assistant" => Some(Message::assistant(m.content.clone())),
                _ => None,
            }),
    );

    restored
}

/// Returns true when a user message is a tool result, tool error, or runtime correction
/// injected by the engine — none of which should be re-injected into a restored context.
fn is_tool_exchange(content: &str) -> bool {
    content.starts_with("=== tool_result:")
        || content.starts_with("=== tool_error:")
        || content.starts_with("[runtime:correction]")
}

fn build_restore_exclusions(messages: &[StoredMessage]) -> Vec<bool> {
    let mut exclude = vec![false; messages.len()];
    for (i, message) in messages.iter().enumerate() {
        if message.role == "user" && is_tool_exchange(&message.content) {
            exclude[i] = true;
            // Drop the preceding assistant message too if it contains no conversational
            // text — only a bare tool call or fabricated result block. Without the result
            // it has no value and would leave an orphaned exchange in context.
            if i > 0 && messages[i - 1].role == "assistant" {
                let prev = messages[i - 1].content.trim_start();
                let is_bare_action = prev.starts_with('[')
                    || prev.starts_with("=== tool_result:")
                    || prev.starts_with("=== tool_error:");
                if is_bare_action {
                    exclude[i - 1] = true;
                }
            }
        }
    }
    exclude
}

fn build_restore_summary(messages: &[StoredMessage], exclude: &[bool]) -> String {
    let mut goals = Vec::new();
    let mut decisions = Vec::new();
    let mut files = Vec::new();
    let mut searches = Vec::new();

    for (message, &is_excluded) in messages.iter().zip(exclude.iter()) {
        if is_excluded {
            continue;
        }
        if !matches!(message.role.as_str(), "user" | "assistant") {
            continue;
        }

        let content = message.content.trim();
        if content.is_empty()
            || content.starts_with("=== tool_result:")
            || content.starts_with("=== tool_error:")
            || content.starts_with("[runtime:correction]")
            || content.starts_with('[')
        {
            continue;
        }

        if message.role == "user" {
            if let Some(goal) = summarized_line(content) {
                push_unique_limited(&mut goals, goal, SUMMARY_GOAL_CAP);
            }
            if let Some(query) = extract_search_query(content) {
                push_unique_limited(&mut searches, query, SUMMARY_SEARCH_CAP);
            }
        }

        if looks_like_decision(content) {
            if let Some(decision) = summarized_line(content) {
                push_unique_limited(&mut decisions, decision, SUMMARY_DECISION_CAP);
            }
        }

        for file in extract_file_references(content) {
            push_unique_limited(&mut files, file, SUMMARY_FILE_CAP);
        }
    }

    format!(
        "[Session Summary]\nGoals:\n{}\nKey Decisions:\n{}\nFiles Referenced:\n{}\nSearches:\n{}",
        render_summary_items(&goals),
        render_summary_items(&decisions),
        render_summary_items(&files),
        render_summary_items(&searches),
    )
}

fn render_summary_items(items: &[String]) -> String {
    if items.is_empty() {
        "* none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("* {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn summarized_line(content: &str) -> Option<String> {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(truncate_chars(&normalized, SUMMARY_ITEM_MAX_CHARS))
    }
}

fn looks_like_decision(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "do not ",
        "don't ",
        "must ",
        "should ",
        "keep ",
        "preserve ",
        "use ",
        "avoid ",
        "instead ",
        "only ",
        "leave ",
        "rebuild ",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn extract_search_query(content: &str) -> Option<String> {
    let line = summarized_line(content)?;
    let lower = line.to_ascii_lowercase();
    for pattern in ["search for ", "search ", "grep ", "ripgrep ", "rg "] {
        if let Some(query) = extract_phrase_suffix(&line, &lower, pattern) {
            let cleaned = query
                .trim()
                .trim_matches(|c: char| matches!(c, '`' | '"' | '\''))
                .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | '!' | '?'))
                .trim();
            if !cleaned.is_empty() {
                return Some(truncate_chars(cleaned, SUMMARY_ITEM_MAX_CHARS));
            }
        }
    }
    None
}

fn extract_phrase_suffix<'a>(original: &'a str, lower: &str, pattern: &str) -> Option<&'a str> {
    let start = lower.find(pattern)?;
    if start > 0 && !lower.as_bytes()[start - 1].is_ascii_whitespace() {
        return None;
    }
    Some(&original[start + pattern.len()..])
}

fn extract_file_references(content: &str) -> Vec<String> {
    let mut files = Vec::new();
    for token in content.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| {
            matches!(
                c,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
            )
        });
        let trimmed = trimmed
            .trim_start_matches("path:")
            .trim_start_matches("file:")
            .trim();
        let cleaned = trimmed.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | '!' | '?'));
        if cleaned.is_empty() || cleaned.contains("://") {
            continue;
        }
        if is_file_reference(cleaned) {
            push_unique_limited(
                &mut files,
                truncate_chars(cleaned, SUMMARY_ITEM_MAX_CHARS),
                SUMMARY_FILE_CAP,
            );
        }
    }
    files
}

fn is_file_reference(candidate: &str) -> bool {
    const FILE_EXTENSIONS: &[&str] = &[
        ".c", ".cc", ".cpp", ".css", ".go", ".h", ".hpp", ".html", ".java", ".js", ".json", ".jsx",
        ".kt", ".lock", ".md", ".py", ".rs", ".scss", ".sh", ".sql", ".toml", ".ts", ".tsx",
        ".txt", ".yaml", ".yml",
    ];

    if candidate == "." || candidate == ".." {
        return false;
    }

    let lower = candidate.to_ascii_lowercase();
    candidate.contains('/') || FILE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn push_unique_limited(items: &mut Vec<String>, value: String, cap: usize) {
    if value.is_empty() || items.len() >= cap || items.iter().any(|existing| existing == &value) {
        return;
    }
    items.push(value);
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::backend::Role;

    fn make_messages() -> Vec<Message> {
        vec![
            Message::system("system prompt"),
            Message::user("hello"),
            Message::assistant("hi there"),
        ]
    }

    #[test]
    fn to_stored_excludes_system_messages() {
        let stored = to_stored(&make_messages());
        assert_eq!(stored.len(), 2);
        assert!(stored.iter().all(|m| m.role != "system"));
    }

    #[test]
    fn to_stored_preserves_role_strings() {
        let stored = to_stored(&make_messages());
        assert_eq!(stored[0].role, "user");
        assert_eq!(stored[1].role, "assistant");
    }

    #[test]
    fn roundtrip_through_stored_messages() {
        use crate::storage::session::{SavedSession, SessionMeta};

        let original = make_messages();
        let stored = to_stored(&original);

        let saved = SavedSession {
            meta: SessionMeta {
                id: "test".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: stored.len(),
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: stored,
        };

        let restored = from_stored(&saved);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].role, Role::User);
        assert_eq!(restored[0].content, "hello");
        assert_eq!(restored[1].role, Role::Assistant);
        assert_eq!(restored[1].content, "hi there");
    }

    #[test]
    fn from_stored_trims_to_restore_window() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let total = RESTORE_WINDOW + 4;
        let messages: Vec<StoredMessage> = (0..total)
            .map(|i| StoredMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("msg {i}"),
            })
            .collect();

        let saved = SavedSession {
            meta: SessionMeta {
                id: "t".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: total,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages,
        };

        let restored = from_stored(&saved);
        assert_eq!(restored.len(), RESTORE_WINDOW + 1);
        assert_eq!(restored[0].role, Role::System);
        assert!(restored[0].content.contains("[Session Summary]"));
        assert_eq!(restored[1].content, "msg 4");
        assert_eq!(
            restored[RESTORE_WINDOW].content,
            format!("msg {}", total - 1)
        );
    }

    #[test]
    fn from_stored_strips_tool_exchange_user_messages() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let tool_result =
            "=== tool_result: read_file ===\nsome file content\n=== /tool_result ===\n\n"
                .to_string();

        let saved = SavedSession {
            meta: SessionMeta {
                id: "t".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: 1,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: vec![StoredMessage {
                role: "user".into(),
                content: tool_result,
            }],
        };

        let restored = from_stored(&saved);
        assert!(
            restored.is_empty(),
            "tool exchange messages must not be injected on restore"
        );
    }

    #[test]
    fn from_stored_strips_adjacent_pure_tool_call_assistant_message() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        // A tool-assisted turn: user prompt → assistant tool call → user tool result
        let saved = SavedSession {
            meta: SessionMeta {
                id: "t".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: 3,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: vec![
                StoredMessage {
                    role: "user".into(),
                    content: "read README.md".into(),
                },
                StoredMessage {
                    role: "assistant".into(),
                    content: "[read_file: README.md]".into(),
                },
                StoredMessage {
                    role: "user".into(),
                    content: "=== tool_result: read_file ===\ncontent\n=== /tool_result ===\n\n"
                        .into(),
                },
            ],
        };

        let restored = from_stored(&saved);
        // Only the original user prompt survives; the tool-call assistant and tool result are stripped
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content, "read README.md");
    }

    #[test]
    fn from_stored_keeps_conversational_assistant_messages() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        // An assistant message that starts with natural language is kept
        let saved = SavedSession {
            meta: SessionMeta {
                id: "t".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: 2,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: vec![
                StoredMessage {
                    role: "user".into(),
                    content: "hello".into(),
                },
                StoredMessage {
                    role: "assistant".into(),
                    content: "Hi there! How can I help?".into(),
                },
            ],
        };

        let restored = from_stored(&saved);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[1].content, "Hi there! How can I help?");
    }

    #[test]
    fn from_stored_strips_fabrication_correction_messages() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let correction = "[runtime:correction] Your response contained a result block which is forbidden. \
            You must emit ONLY a tool call tag (e.g. [read_file: path]) or answer directly in plain text. \
            Output the tool call tag now, with no other text.".to_string();

        let saved = SavedSession {
            meta: SessionMeta {
                id: "t".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: 3,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: vec![
                StoredMessage {
                    role: "user".into(),
                    content: "list the files".into(),
                },
                StoredMessage {
                    role: "assistant".into(),
                    content: "=== tool_result: list_dir ===\nfoo\n=== /tool_result ===".into(),
                },
                StoredMessage {
                    role: "user".into(),
                    content: correction,
                },
            ],
        };

        let restored = from_stored(&saved);
        // Original user message survives; correction and fabricated assistant message are stripped
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content, "list the files");
    }

    #[test]
    fn from_stored_skips_unknown_roles() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let saved = SavedSession {
            meta: SessionMeta {
                id: "test".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: 1,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: vec![StoredMessage {
                role: "unknown_role".into(),
                content: "some content".into(),
            }],
        };

        let restored = from_stored(&saved);
        assert!(restored.is_empty());
    }

    #[test]
    fn from_stored_injects_summary_as_system_message_for_trimmed_history() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let mut messages = vec![
            StoredMessage {
                role: "user".into(),
                content: "search for RESTORE_WINDOW in src/app/session.rs".into(),
            },
            StoredMessage {
                role: "assistant".into(),
                content: "We should keep restore filtering before summarization.".into(),
            },
        ];
        messages.extend((0..RESTORE_WINDOW).map(|i| StoredMessage {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("tail {i}"),
        }));

        let saved = SavedSession {
            meta: SessionMeta {
                id: "summary".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: messages.len(),
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages,
        };

        let restored = from_stored(&saved);
        assert_eq!(restored[0].role, Role::System);
        assert!(restored[0].content.contains("[Session Summary]"));
        assert!(restored[0].content.contains("Goals:"));
        assert!(restored[0].content.contains("Key Decisions:"));
        assert!(restored[0].content.contains("Files Referenced:"));
        assert!(restored[0].content.contains("Searches:"));
        assert!(restored[0]
            .content
            .contains("RESTORE_WINDOW in src/app/session.rs"));
        assert!(restored[0].content.contains("src/app/session.rs"));
        assert!(restored[0]
            .content
            .contains("We should keep restore filtering before summarization."));
    }

    #[test]
    fn from_stored_does_not_inject_summary_when_message_count_matches_window() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let messages: Vec<StoredMessage> = (0..RESTORE_WINDOW)
            .map(|i| StoredMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("msg {i}"),
            })
            .collect();

        let saved = SavedSession {
            meta: SessionMeta {
                id: "exact".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: messages.len(),
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages,
        };

        let restored = from_stored(&saved);
        assert_eq!(restored.len(), RESTORE_WINDOW);
        assert!(restored.iter().all(|message| message.role != Role::System));
    }

    #[test]
    fn from_stored_short_sessions_do_not_get_summary_blocks() {
        let restored = from_stored(&SavedSession {
            meta: SessionMeta {
                id: "short".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: 2,
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages: vec![
                StoredMessage {
                    role: "user".into(),
                    content: "hello".into(),
                },
                StoredMessage {
                    role: "assistant".into(),
                    content: "hi there".into(),
                },
            ],
        });

        assert_eq!(restored.len(), 2);
        assert!(restored.iter().all(|message| message.role != Role::System));
    }

    #[test]
    fn from_stored_excludes_stripped_tool_exchanges_from_summary() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let mut messages = vec![
            StoredMessage {
                role: "user".into(),
                content: "please investigate the restore flow".into(),
            },
            StoredMessage {
                role: "assistant".into(),
                content: "[read_file: secret.rs]".into(),
            },
            StoredMessage {
                role: "user".into(),
                content: "=== tool_result: read_file ===\npath: secret.rs\nsuper secret\n=== /tool_result ===\n\n"
                    .into(),
            },
        ];
        messages.extend((0..RESTORE_WINDOW).map(|i| StoredMessage {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("tail {i}"),
        }));

        let saved = SavedSession {
            meta: SessionMeta {
                id: "strip-summary".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: messages.len(),
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages,
        };

        let restored = from_stored(&saved);
        let summary = &restored[0];
        assert_eq!(summary.role, Role::System);
        assert!(summary
            .content
            .contains("please investigate the restore flow"));
        assert!(!summary.content.contains("secret.rs"));
        assert!(!summary.content.contains("super secret"));
        assert!(!summary.content.contains("tool_result"));
        assert!(!summary.content.contains("[read_file:"));
    }

    #[test]
    fn restore_summary_is_not_persisted() {
        use crate::storage::session::{SavedSession, SessionMeta, StoredMessage};

        let mut messages = vec![
            StoredMessage {
                role: "user".into(),
                content: "search for RESTORE_WINDOW in src/app/session.rs".into(),
            },
            StoredMessage {
                role: "assistant".into(),
                content: "We should keep restore filtering before summarization.".into(),
            },
        ];
        messages.extend((0..RESTORE_WINDOW).map(|i| StoredMessage {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("tail {i}"),
        }));

        let saved = SavedSession {
            meta: SessionMeta {
                id: "persist".into(),
                project_root: Some("/tmp/project".into()),
                created_at: 0,
                updated_at: 0,
                message_count: messages.len(),
                last_read_file: None,
                last_search_query: None,
                last_search_scope: None,
            },
            messages,
        };

        let restored = from_stored(&saved);
        let stored = to_stored(&restored);
        assert_eq!(stored.len(), RESTORE_WINDOW);
        assert!(stored.iter().all(|message| message.role != "system"));
        assert!(stored
            .iter()
            .all(|message| !message.content.contains("[Session Summary]")));
    }

    fn temp_project_root() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn canonical_project_root(dir: &tempfile::TempDir) -> ProjectRoot {
        ProjectRoot::new(dir.path().to_path_buf()).unwrap()
    }

    fn session_db_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("sessions.db")
    }

    #[test]
    fn open_or_restore_restores_session_when_project_root_matches() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let root_dir = temp_project_root();
        let root = canonical_project_root(&root_dir);
        let db_path = session_db_path(&db_dir);

        let store = SessionStore::open(&db_path).unwrap();
        let meta = store.create(root.path()).unwrap();
        store
            .save(
                &meta.id,
                &[
                    StoredMessage {
                        role: "user".into(),
                        content: "hello".into(),
                    },
                    StoredMessage {
                        role: "assistant".into(),
                        content: "hi there".into(),
                    },
                ],
                None,
                None,
                None,
            )
            .unwrap();

        let (_session, history, _anchors) =
            ActiveSession::open_or_restore(&db_path, &root).unwrap();

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "hello");
        assert_eq!(history[1].content, "hi there");
        assert_eq!(
            SessionStore::open(&db_path).unwrap().list().unwrap().len(),
            1
        );
    }

    #[test]
    fn open_or_restore_creates_new_session_when_project_root_differs() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let original_root_dir = temp_project_root();
        let current_root_dir = temp_project_root();
        let original_root = canonical_project_root(&original_root_dir);
        let current_root = canonical_project_root(&current_root_dir);
        let db_path = session_db_path(&db_dir);

        let store = SessionStore::open(&db_path).unwrap();
        let original = store.create(original_root.path()).unwrap();
        store
            .save(
                &original.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "stale history".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        let (_session, history, _anchors) =
            ActiveSession::open_or_restore(&db_path, &current_root).unwrap();

        assert!(history.is_empty());

        let store = SessionStore::open(&db_path).unwrap();
        let sessions = store.list().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_ne!(sessions[0].id, original.id);
        assert_eq!(
            sessions[0].project_root.as_deref(),
            Some(current_root.path().to_string_lossy().as_ref())
        );
        assert_eq!(sessions[0].message_count, 0);
    }

    #[test]
    fn open_or_restore_restores_project_a_session_when_project_b_is_more_recent() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let root_a_dir = temp_project_root();
        let root_b_dir = temp_project_root();
        let root_a = canonical_project_root(&root_a_dir);
        let root_b = canonical_project_root(&root_b_dir);
        let db_path = session_db_path(&db_dir);

        let store = SessionStore::open(&db_path).unwrap();
        let meta_a = store.create(root_a.path()).unwrap();
        let meta_b = store.create(root_b.path()).unwrap();

        store
            .save(
                &meta_a.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "project a history".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();
        // Save to B last so it is globally most recent
        store
            .save(
                &meta_b.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "project b history".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        // Returning to project A must restore A's session, not start fresh
        let (_session, history, _anchors) =
            ActiveSession::open_or_restore(&db_path, &root_a).unwrap();

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "project a history");

        // No new session should have been created
        let store = SessionStore::open(&db_path).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn open_or_restore_creates_new_session_when_project_root_is_missing() {
        use rusqlite::Connection;

        let db_dir = tempfile::TempDir::new().unwrap();
        let root_dir = temp_project_root();
        let root = canonical_project_root(&root_dir);
        let db_path = session_db_path(&db_dir);

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE sessions (
                id          TEXT PRIMARY KEY,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL,
                msg_count   INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE session_messages (
                session_id  TEXT NOT NULL,
                seq         INTEGER NOT NULL,
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                PRIMARY KEY (session_id, seq)
            );

            CREATE INDEX idx_sessions_updated
                ON sessions(updated_at DESC);

            CREATE INDEX idx_session_messages_lookup
                ON session_messages(session_id, seq);

            PRAGMA user_version = 1;
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, created_at, updated_at, msg_count)
             VALUES (?1, ?2, ?2, 1)",
            ("legacy", 1_i64),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_messages (session_id, seq, role, content)
             VALUES (?1, 0, ?2, ?3)",
            ("legacy", "user", "legacy history"),
        )
        .unwrap();
        drop(conn);

        let (_session, history, _anchors) =
            ActiveSession::open_or_restore(&db_path, &root).unwrap();
        assert!(history.is_empty());

        let store = SessionStore::open(&db_path).unwrap();
        let legacy = store.load("legacy").unwrap().unwrap();
        assert_eq!(legacy.meta.project_root, None);

        let sessions = store.list().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0].project_root.as_deref(),
            Some(root.path().to_string_lossy().as_ref())
        );
        assert_eq!(sessions[0].message_count, 0);
    }

    #[test]
    fn anchors_restored_after_session_restore() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let root_dir = temp_project_root();
        let root = canonical_project_root(&root_dir);
        let db_path = session_db_path(&db_dir);

        let store = SessionStore::open(&db_path).unwrap();
        let meta = store.create(root.path()).unwrap();
        store
            .save(
                &meta.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "hello".into(),
                }],
                Some("src/lib.rs"),
                Some("fn main"),
                Some("src/"),
            )
            .unwrap();

        let (_session, _history, anchors) =
            ActiveSession::open_or_restore(&db_path, &root).unwrap();

        assert_eq!(anchors.0.as_deref(), Some("src/lib.rs"));
        assert_eq!(anchors.1.as_deref(), Some("fn main"));
        assert_eq!(anchors.2.as_deref(), Some("src/"));
    }

    #[test]
    fn missing_anchor_data_in_session_defaults_to_none() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let root_dir = temp_project_root();
        let root = canonical_project_root(&root_dir);
        let db_path = session_db_path(&db_dir);

        let store = SessionStore::open(&db_path).unwrap();
        let meta = store.create(root.path()).unwrap();
        store.save(&meta.id, &[], None, None, None).unwrap();

        let (_session, _history, anchors) =
            ActiveSession::open_or_restore(&db_path, &root).unwrap();

        assert_eq!(anchors.0, None);
        assert_eq!(anchors.1, None);
        assert_eq!(anchors.2, None);
    }

    #[test]
    fn list_for_project_returns_only_current_project_sessions() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let root_a_dir = temp_project_root();
        let root_b_dir = temp_project_root();
        let root_a = canonical_project_root(&root_a_dir);
        let root_b = canonical_project_root(&root_b_dir);
        let db_path = session_db_path(&db_dir);

        let (session_a, _history, _anchors) =
            ActiveSession::open_or_restore(&db_path, &root_a).unwrap();
        let store = SessionStore::open(&db_path).unwrap();
        let other = store.create(root_b.path()).unwrap();
        store
            .save(
                &other.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "project b".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        let listed = session_a.list_for_project().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].project_root.as_deref(),
            Some(root_a.path().to_string_lossy().as_ref())
        );
    }

    #[test]
    fn clear_for_project_removes_old_sessions_and_starts_fresh_one() {
        let db_dir = tempfile::TempDir::new().unwrap();
        let root_a_dir = temp_project_root();
        let root_b_dir = temp_project_root();
        let root_a = canonical_project_root(&root_a_dir);
        let root_b = canonical_project_root(&root_b_dir);
        let db_path = session_db_path(&db_dir);

        let (mut session_a, _history, _anchors) =
            ActiveSession::open_or_restore(&db_path, &root_a).unwrap();
        session_a.begin_new().unwrap();

        let store = SessionStore::open(&db_path).unwrap();
        let other = store.create(root_b.path()).unwrap();
        store
            .save(
                &other.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "project b".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        session_a.clear_for_project().unwrap();

        let current_sessions = session_a.list_for_project().unwrap();
        assert_eq!(current_sessions.len(), 1);
        assert_eq!(current_sessions[0].message_count, 0);
        assert_eq!(
            current_sessions[0].project_root.as_deref(),
            Some(root_a.path().to_string_lossy().as_ref())
        );

        let store = SessionStore::open(&db_path).unwrap();
        let other_sessions = store
            .list_for_project(root_b.path().to_string_lossy().as_ref())
            .unwrap();
        assert_eq!(other_sessions.len(), 1);
        assert_eq!(other_sessions[0].id, other.id);
    }
}
