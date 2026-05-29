use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::config::{AllowedCommandTool, Config};
use crate::app::paths::AppPaths;
use crate::app::AppContext;
use crate::app::Result;
use crate::runtime::{AnswerSource, RuntimeEvent, RuntimeRequest};
use crate::storage::session::SessionMeta;

use super::commands;
use super::render::render;
use super::state::AppState;

pub(crate) fn run_app(
    stdout: &mut io::Stdout,
    config: &Config,
    paths: &AppPaths,
    app: &mut AppContext,
) -> Result<()> {
    let mut state = AppState::new(config, paths);

    loop {
        render(stdout, &mut state)?;

        if state.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                    handle_key_event(stdout, &mut state, app, config, key)?
                }
                Event::Paste(text) => state.insert_str(&text),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn handle_key_event(
    stdout: &mut io::Stdout,
    state: &mut AppState,
    app: &mut AppContext,
    config: &Config,
    key: KeyEvent,
) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }
        (KeyCode::Enter, _) => {
            if let Some(input) = state.submit_input() {
                match commands::parse(&input) {
                    None => submit_to_app(stdout, state, app, input)?,
                    Some(Ok(cmd)) => handle_command(stdout, state, app, cmd)?,
                    Some(Err(commands::ParseError::UnknownCommand)) => {
                        match resolve_custom_command(config, &input) {
                            None => state.add_system_message(
                                commands::ParseError::UnknownCommand.user_message(),
                            ),
                            Some(Err(msg)) => state.add_system_message(msg),
                            Some(Ok(req)) => {
                                dispatch_command_runtime_request(stdout, state, app, req)?
                            }
                        }
                    }
                    Some(Err(e)) => state.add_system_message(e.user_message()),
                }
            }
        }
        (KeyCode::Backspace, _) => state.delete_char_before(),
        (KeyCode::Left, _) => state.cursor_left(),
        (KeyCode::Right, _) => state.cursor_right(),
        (KeyCode::Home, _) => state.cursor_home(),
        (KeyCode::End, _) => state.cursor_end(),
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if let Some(prompt) = &state.last_prompt {
                let path = std::env::temp_dir().join("thunk_last_prompt.txt");
                dump_prompt_to_file(&path, prompt);
                state.set_status(&format!("prompt dumped to {}", path.display()));
            } else {
                state.set_status("no prompt captured yet");
            }
        }
        (KeyCode::Up, _) => state.scroll_up(1),
        (KeyCode::Down, _) => state.scroll_down(1),
        (KeyCode::PageUp, _) => state.scroll_up(10),
        (KeyCode::PageDown, _) => state.scroll_down(10),
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => state.toggle_file_expand(),
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => state.insert_char(c),
        _ => {}
    }

    Ok(())
}

// Used by Approve and Reject: applies Failed event before propagating render errors.
// submit_to_app has a different post-handle ordering and is kept separate.
fn dispatch_command_runtime_request(
    stdout: &mut io::Stdout,
    state: &mut AppState,
    app: &mut AppContext,
    req: RuntimeRequest,
) -> Result<()> {
    let mut render_error = None;
    if let Err(e) = app.handle(req, &mut |event| {
        if render_error.is_some() {
            return;
        }
        apply_runtime_event(state, event);
        if let Err(e) = render(stdout, state) {
            render_error = Some(e);
        }
    }) {
        apply_runtime_event(
            state,
            RuntimeEvent::Failed {
                message: e.to_string(),
            },
        );
    }
    if let Some(e) = render_error {
        return Err(e);
    }
    Ok(())
}

fn submit_to_app(
    stdout: &mut io::Stdout,
    state: &mut AppState,
    app: &mut AppContext,
    prompt: String,
) -> Result<()> {
    state.add_user_message(prompt.clone());
    let mut render_error = None;

    let handle_result = app.handle(RuntimeRequest::Submit { text: prompt }, &mut |event| {
        if render_error.is_some() {
            return;
        }
        apply_runtime_event(state, event);
        if let Err(e) = render(stdout, state) {
            render_error = Some(e);
        }
    });

    if let Some(e) = render_error {
        return Err(e);
    }

    if let Err(e) = handle_result {
        apply_runtime_event(
            state,
            RuntimeEvent::Failed {
                message: e.to_string(),
            },
        );
    }

    Ok(())
}

enum CommandAction {
    Quit,
    ShowHelp,
    ClearSession,
    ListSessions,
    ClearProjectSessions,
    Runtime(RuntimeRequest),
}

fn resolve_command(cmd: commands::Command) -> CommandAction {
    match cmd {
        commands::Command::Help => CommandAction::ShowHelp,
        commands::Command::Quit => CommandAction::Quit,
        commands::Command::Clear => CommandAction::ClearSession,
        commands::Command::Approve => CommandAction::Runtime(RuntimeRequest::Approve),
        commands::Command::Reject => CommandAction::Runtime(RuntimeRequest::Reject),
        commands::Command::Last => CommandAction::Runtime(RuntimeRequest::QueryLast),
        commands::Command::Anchors => CommandAction::Runtime(RuntimeRequest::QueryAnchors),
        commands::Command::History => CommandAction::Runtime(RuntimeRequest::QueryHistory),
        commands::Command::Read(path) => CommandAction::Runtime(RuntimeRequest::ReadFile { path }),
        commands::Command::Search(query) => {
            CommandAction::Runtime(RuntimeRequest::SearchCode { query })
        }
        commands::Command::Sessions => CommandAction::ListSessions,
        commands::Command::SessionClear => CommandAction::ClearProjectSessions,
        commands::Command::Undo => CommandAction::Runtime(RuntimeRequest::Undo),
        commands::Command::ProvidersList => CommandAction::Runtime(RuntimeRequest::ProvidersList),
        commands::Command::ProvidersUse(name) => {
            CommandAction::Runtime(RuntimeRequest::ProvidersUse { name })
        }
        commands::Command::GitBranch => CommandAction::Runtime(RuntimeRequest::GitBranch),
        commands::Command::GitStatus => CommandAction::Runtime(RuntimeRequest::GitStatus),
        commands::Command::GitDiff => CommandAction::Runtime(RuntimeRequest::GitDiff),
        commands::Command::GitLog => CommandAction::Runtime(RuntimeRequest::GitLog),
        commands::Command::Ls(path) => CommandAction::Runtime(RuntimeRequest::ListDir { path }),
        commands::Command::LspStatus => CommandAction::Runtime(RuntimeRequest::LspStatus),
        commands::Command::IndexBuild { large } => {
            CommandAction::Runtime(RuntimeRequest::IndexBuild { large })
        }
        commands::Command::IndexStatus => CommandAction::Runtime(RuntimeRequest::IndexStatus),
    }
}

fn handle_command(
    stdout: &mut io::Stdout,
    state: &mut AppState,
    app: &mut AppContext,
    cmd: commands::Command,
) -> Result<()> {
    match resolve_command(cmd) {
        CommandAction::ShowHelp => {
            state.add_system_message(
                "Commands:\n\n  Navigation\n    /read <path>          read a file\n    /search <query>       search code\n    /last                 show last response\n    /anchors              show anchor state\n    /history              conversation history\n\n  Git\n    /git status           git status\n    /git diff             git diff\n    /git log              git log\n    /git branch           current branch\n\n  Session\n    /sessions             list project sessions\n    /session clear        delete sessions and start fresh\n    /clear                clear transcript history\n\n  Actions\n    /approve              confirm pending action\n    /reject               cancel pending action\n    /undo                 revert last mutation\n\n  Providers\n    /providers list       list available providers\n    /providers use <name> switch provider (session-only)\n\n  Index\n    /index status         symbol count and last build time\n    /index build          build symbol index\n    /index build --large  build without file-count guard\n\n  General\n    /help                 show this message\n    /quit                 exit",
            );
        }
        CommandAction::Quit => {
            state.should_quit = true;
        }
        CommandAction::ClearSession => {
            state.clear_messages();
            if let Err(e) = app.reset() {
                state.add_system_message(format!("session reset failed: {e}"));
            }
        }
        CommandAction::ListSessions => match app.list_sessions() {
            Ok(sessions) => state.add_system_message(format_sessions_list(&sessions)),
            Err(e) => {
                state.set_status("error");
                state.add_system_message(format!("session list failed: {e}"));
            }
        },
        CommandAction::ClearProjectSessions => {
            state.clear_messages();
            match app.clear_sessions() {
                Ok(()) => {
                    state.set_status("ready");
                    state.add_system_message(
                        "current project sessions cleared; started fresh session",
                    );
                }
                Err(e) => {
                    state.set_status("error");
                    state.add_system_message(format!("session clear failed: {e}"));
                }
            }
        }
        CommandAction::Runtime(req) => {
            dispatch_command_runtime_request(stdout, state, app, req)?;
        }
    }
    Ok(())
}

/// Resolves a raw input string against the custom command definitions in config.
///
/// Returns:
/// - `None`           — no custom command with this name; caller shows "unknown command"
/// - `Some(Err(msg))` — command found but argument is missing
/// - `Some(Ok(req))`  — resolved to a RuntimeRequest ready for dispatch
fn resolve_custom_command(
    config: &Config,
    input: &str,
) -> Option<std::result::Result<RuntimeRequest, String>> {
    let trimmed = input.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let slash_name = parts.next()?;
    let name = slash_name.strip_prefix('/')?;
    let def = config.commands.get(name)?;

    let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());
    let arg_str = match arg {
        Some(a) => a.to_string(),
        None => return Some(Err(format!("/{name}: argument required"))),
    };

    let value = def.template.replace("{input}", &arg_str);
    let req = match def.tool {
        AllowedCommandTool::ReadFile => RuntimeRequest::ReadFile { path: value },
        AllowedCommandTool::SearchCode => RuntimeRequest::SearchCode { query: value },
    };
    Some(Ok(req))
}

/// Converts a raw tool_result InfoMessage into a compact human-readable summary.
/// Non-tool-result InfoMessages (query output, error text, etc.) pass through unchanged.
fn summarize_command_output(text: &str) -> String {
    let Some(after_prefix) = text.strip_prefix("=== tool_result: ") else {
        return text.to_string();
    };
    let Some(name_end) = after_prefix.find(" ===\n") else {
        return text.to_string();
    };
    let tool_name = &after_prefix[..name_end];
    let header_len = "=== tool_result: ".len() + name_end + " ===\n".len();
    let raw_body = text.get(header_len..).unwrap_or("").trim_end();
    let body = raw_body
        .strip_suffix("=== /tool_result ===")
        .unwrap_or(raw_body)
        .trim_end();

    match tool_name {
        "read_file" => {
            let first = body.lines().next().unwrap_or("");
            match parse_read_file_header(first) {
                Some((n, false)) => format!("read: {n} lines"),
                Some((n, true)) => format!("read: {n} lines (truncated)"),
                None => "read: done".to_string(),
            }
        }
        "search_code" => {
            if body.starts_with("No matches found.") {
                return "search: no matches".to_string();
            }
            let first = body.lines().next().unwrap_or("");
            // Truncated header: "[showing first M of N matches — ...]"
            if let Some(inner) = first.strip_prefix("[showing first ") {
                if let Some(of_pos) = inner.find(" of ") {
                    let m = &inner[..of_pos];
                    let after_of = &inner[of_pos + " of ".len()..];
                    let n = after_of.split_whitespace().next().unwrap_or("?");
                    return format!("search: {n} matches (showing {m})");
                }
            }
            // Untruncated: match lines are indented "  <line_num>: <content>"
            let count = body
                .lines()
                .filter(|l| {
                    l.starts_with("  ")
                        && l.trim_start()
                            .chars()
                            .next()
                            .map(|c| c.is_ascii_digit())
                            .unwrap_or(false)
                })
                .count();
            if count > 0 {
                format!("search: {count} matches")
            } else {
                "search: done".to_string()
            }
        }
        "git_status" | "git_diff" | "git_log" => body.to_string(),
        "git_branch" => {
            if body == "No branches found." {
                return "git branch: no branches".to_string();
            }
            let current = body
                .lines()
                .find(|l| l.starts_with("current: "))
                .and_then(|l| l.strip_prefix("current: "))
                .unwrap_or("unknown");
            format!("git branch: {current}")
        }
        "list_dir" => {
            let dir_count = body.lines().filter(|l| l.starts_with("dir")).count();
            let file_count = body.lines().filter(|l| l.starts_with("file")).count();
            format!("ls: {dir_count} dirs, {file_count} files")
        }
        _ => text.to_string(),
    }
}

/// Parses the first line of a read_file body: "[N lines]" or "[N lines — showing first M]".
/// Returns `(total_lines, is_truncated)` or `None` if the format is not recognised.
fn parse_read_file_header(line: &str) -> Option<(usize, bool)> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let truncated = inner.contains(" — ");
    let count_str = inner.split(" — ").next()?.split_whitespace().next()?;
    let n: usize = count_str.parse().ok()?;
    Some((n, truncated))
}

fn format_sessions_list(sessions: &[SessionMeta]) -> String {
    if sessions.is_empty() {
        return "current project sessions: none".to_string();
    }

    let mut lines = vec!["current project sessions:".to_string()];
    for session in sessions {
        lines.push(format!(
            "{}  |  {}  |  {} messages",
            session.id,
            format_session_updated_at(session.updated_at),
            session.message_count
        ));
    }
    lines.join("\n")
}

fn format_session_updated_at(updated_at: u64) -> String {
    let seconds = normalize_session_timestamp_seconds(updated_at);
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn normalize_session_timestamp_seconds(timestamp: u64) -> i64 {
    if timestamp >= 1_000_000_000_000_000 {
        (timestamp / 1_000_000_000) as i64
    } else if timestamp >= 10_000_000_000 {
        (timestamp / 1_000) as i64
    } else {
        timestamp as i64
    }
}

fn civil_from_unix_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn dump_prompt_to_file(path: &std::path::Path, prompt: &str) {
    let _ = std::fs::write(path, prompt);
}

/// Decodes a v2 edit_file payload and returns a diff approval message, or None if the
/// payload doesn't match the expected format (caller falls back to the generic summary).
///
/// Payload format: `v2\x00{absolute_path}\x00{display_path}\x00{search_text}\x00{replace_text}`
fn format_edit_approval(payload: &str) -> Option<String> {
    let parts: Vec<&str> = payload.split('\x00').collect();
    if parts.len() < 5 || parts[0] != "v2" {
        return None;
    }
    let display_path = parts[2];
    let search_text = parts[3];
    let replace_text = parts[4];
    let diff_lines = search_text
        .lines()
        .map(|l| format!("- {l}"))
        .chain(replace_text.lines().map(|l| format!("+ {l}")))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "[approval required] edit {display_path}\n{diff_lines}\ntype /approve to confirm or /reject to cancel"
    ))
}

fn apply_runtime_event(state: &mut AppState, event: RuntimeEvent) {
    match event {
        RuntimeEvent::ActivityChanged(activity) => state.set_status(&activity.label()),
        RuntimeEvent::AssistantMessageStarted => state.begin_assistant_message(),
        RuntimeEvent::AssistantMessageChunk(chunk) => state.append_assistant_chunk(&chunk),
        RuntimeEvent::AssistantMessageFinished => {}
        RuntimeEvent::ToolCallStarted { name } => {
            state.add_tool_message(format!("tool: {name}"));
        }
        RuntimeEvent::ToolCallFinished { name, summary } => match summary {
            // FileReadFinished fires for every successful read_file and adds the
            // canonical "read {path} ({n} lines) — Ctrl+O to expand" message.
            // Suppress the compact ToolCallFinished duplicate to keep a single summary.
            Some(_) if name == "read_file" => {}
            Some(s) => state.add_tool_message(s),
            None => state.add_tool_message(format!("tool failed: {name}")),
        },
        RuntimeEvent::AnswerReady(source) => {
            state.set_status("ready");
            if let AnswerSource::ToolLimitReached = source {
                state.add_system_message("Tool limit reached. Response may be incomplete.");
            }
        }
        RuntimeEvent::Failed { message } => {
            state.set_status("error");
            state.add_error_message(message);
        }
        RuntimeEvent::ApprovalRequired { pending, evidence } => {
            let message = if pending.tool_name == "edit_file" {
                format_edit_approval(&pending.payload).unwrap_or_else(|| {
                    let evidence_str = if evidence.is_empty() {
                        String::new()
                    } else {
                        format!("\nEvidence: {}", evidence.join(" | "))
                    };
                    format!(
                        "[approval required] {}{} — type /approve to confirm or /reject to cancel",
                        pending.summary, evidence_str
                    )
                })
            } else {
                let evidence_str = if evidence.is_empty() {
                    String::new()
                } else {
                    format!("\nEvidence: {}", evidence.join(" | "))
                };
                format!(
                    "[approval required] {}{} — type /approve to confirm or /reject to cancel",
                    pending.summary, evidence_str
                )
            };
            state.add_alert_message(message);
            state.set_status("awaiting approval");
        }
        RuntimeEvent::InfoMessage(text) => {
            state.add_system_message(summarize_command_output(&text))
        }
        RuntimeEvent::PromptAssembled(prompt) => state.set_last_prompt(prompt),
        RuntimeEvent::SystemMessage(text) => state.add_system_message(text),
        RuntimeEvent::FileReadFinished {
            path,
            line_count,
            content: _,
        } => {
            state.add_system_message(format!(
                "read {path} ({line_count} lines) — Ctrl+O to expand"
            ));
        }
        RuntimeEvent::DirectReadCompleted => {
            let message_index = state.messages.len() - 1;
            state.store_file_read(message_index);
        }
        RuntimeEvent::ContextUsage {
            prompt_tokens,
            context_window_tokens,
        } => {
            let pct = (prompt_tokens * 100 / u64::from(context_window_tokens)).min(100) as u8;
            state.context_pct = Some(pct);
        }
        // Advisory only — absorbed by the logging layer before reaching here.
        RuntimeEvent::BackendTiming { .. } => {}
        RuntimeEvent::BackendTokenCounts { .. } => {}
        RuntimeEvent::RuntimeTrace(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;

    use tempfile::TempDir;

    use crate::app::config::Config;
    use crate::app::paths::AppPaths;
    use crate::app::session::ActiveSession;
    use crate::app::AppContext;
    use crate::llm::providers::build_backend;
    use crate::runtime::{ProjectRoot, RuntimeEvent, RuntimeRequest};
    use crate::storage::session::{SessionStore, StoredMessage};
    use crate::tools::default_registry;

    use super::{
        apply_runtime_event, format_edit_approval, format_session_updated_at, format_sessions_list,
        handle_command, parse_read_file_header, summarize_command_output,
    };
    use crate::tui::commands::Command;
    use crate::tui::state::AppState;

    fn tool_result(name: &str, body: &str) -> String {
        format!("=== tool_result: {name} ===\n{body}\n=== /tool_result ===\n\n")
    }

    // parse_read_file_header

    // format_edit_approval

    #[test]
    fn edit_approval_renders_diff_with_path() {
        let payload = "v2\x00/abs/src/main.rs\x00src/main.rs\x00old line\x00new line";
        let msg = format_edit_approval(payload).unwrap();
        assert!(msg.starts_with("[approval required] edit src/main.rs\n"));
        assert!(msg.contains("- old line"));
        assert!(msg.contains("+ new line"));
        assert!(msg.ends_with("\ntype /approve to confirm or /reject to cancel"));
    }

    #[test]
    fn edit_approval_multiline_diff() {
        let payload = "v2\x00/abs/lib.rs\x00lib.rs\x00fn old() {}\nfn also_old() {}\x00fn new() {}\nfn also_new() {}";
        let msg = format_edit_approval(payload).unwrap();
        assert!(msg.contains("- fn old() {}"));
        assert!(msg.contains("- fn also_old() {}"));
        assert!(msg.contains("+ fn new() {}"));
        assert!(msg.contains("+ fn also_new() {}"));
    }

    #[test]
    fn edit_approval_returns_none_for_malformed_payload() {
        assert!(format_edit_approval("not_v2\x00a\x00b\x00c\x00d").is_none());
        assert!(format_edit_approval("v2\x00only_three\x00parts").is_none());
        assert!(format_edit_approval("no_nulls_at_all").is_none());
    }

    #[test]
    fn parses_untruncated_header() {
        assert_eq!(parse_read_file_header("[42 lines]"), Some((42, false)));
    }

    #[test]
    fn parses_truncated_header() {
        assert_eq!(
            parse_read_file_header("[300 lines — showing first 200]"),
            Some((300, true))
        );
    }

    #[test]
    fn rejects_malformed_header() {
        assert_eq!(parse_read_file_header("no brackets here"), None);
        assert_eq!(parse_read_file_header("[not a number lines]"), None);
    }

    // summarize_command_output — pass-through cases

    #[test]
    fn non_tool_result_passes_through_unchanged() {
        let msg = "no conversation history";
        assert_eq!(summarize_command_output(msg), msg);
    }

    #[test]
    fn query_output_passes_through_unchanged() {
        let msg = "last search: fn handle";
        assert_eq!(summarize_command_output(msg), msg);
    }

    // summarize_command_output — read_file

    #[test]
    fn read_file_untruncated_shows_line_count() {
        let body = "[42 lines]\nfn main() {}\n";
        let summary = summarize_command_output(&tool_result("read_file", body));
        assert_eq!(summary, "read: 42 lines");
    }

    #[test]
    fn read_file_truncated_shows_line_count_and_truncated() {
        let body =
            "[300 lines — showing first 200]\nfn main() {}\n[truncated: 100 lines not shown]";
        let summary = summarize_command_output(&tool_result("read_file", body));
        assert_eq!(summary, "read: 300 lines (truncated)");
    }

    // summarize_command_output — search_code

    #[test]
    fn search_no_matches_shows_no_matches() {
        let body = "No matches found.";
        let summary = summarize_command_output(&tool_result("search_code", body));
        assert_eq!(summary, "search: no matches");
    }

    #[test]
    fn search_truncated_shows_total_and_shown() {
        let body = "[showing first 15 of 42 matches — read a specific matched file with read_file]\nsrc/main.rs (3 matches)\n  12: fn handle()";
        let summary = summarize_command_output(&tool_result("search_code", body));
        assert_eq!(summary, "search: 42 matches (showing 15)");
    }

    #[test]
    fn search_untruncated_counts_match_lines() {
        let body =
            "src/main.rs (2 matches)\n  12: fn handle_request() {}\n  45: fn handle_response() {}";
        let summary = summarize_command_output(&tool_result("search_code", body));
        assert_eq!(summary, "search: 2 matches");
    }

    #[test]
    fn unknown_tool_passes_through_raw() {
        let raw = tool_result("unknown_tool", "some output");
        assert_eq!(summarize_command_output(&raw), raw);
    }

    #[test]
    fn summarize_git_branch_shows_current_branch() {
        let body = "current: dev\nbranches: dev, main";
        let raw = tool_result("git_branch", body);
        assert_eq!(summarize_command_output(&raw), "git branch: dev");
    }

    #[test]
    fn summarize_list_dir_shows_counts() {
        let body = "dir   src\ndir   docs\nfile  README.md\nfile  Cargo.toml\nfile  main.rs";
        let raw = tool_result("list_dir", body);
        assert_eq!(summarize_command_output(&raw), "ls: 2 dirs, 3 files");
    }

    #[test]
    fn session_timestamp_formats_as_utc_datetime() {
        let ts = 1_778_198_400_000_000_000_u64;
        assert_eq!(format_session_updated_at(ts), "2026-05-08 00:00:00 UTC");
    }

    #[test]
    fn sessions_list_includes_id_timestamp_and_message_count() {
        let sessions = vec![crate::storage::session::SessionMeta {
            id: "abc123".into(),
            project_root: Some("/tmp/project".into()),
            created_at: 0,
            updated_at: 1_778_198_400_000_000_000,
            message_count: 3,
            last_read_file: None,
            last_search_query: None,
            last_search_scope: None,
        }];

        let text = format_sessions_list(&sessions);
        assert!(text.contains("current project sessions:"));
        assert!(text.contains("abc123"));
        assert!(text.contains("2026-05-08 00:00:00 UTC"));
        assert!(text.contains("3 messages"));
    }

    #[test]
    fn session_clear_removes_old_project_sessions_and_leaves_fresh_active_session() {
        let mut harness = TestHarness::new();
        let mut stdout = io::stdout();
        let mut state = AppState::new(&harness.config, &harness.paths);
        state.add_user_message("stale user message");
        state.add_assistant_message("stale assistant message");

        harness
            .app
            .handle(
                RuntimeRequest::Submit {
                    text: "before clear".into(),
                },
                &mut |_| {},
            )
            .unwrap();
        harness.app.reset().unwrap();
        harness
            .app
            .handle(
                RuntimeRequest::Submit {
                    text: "second session".into(),
                },
                &mut |_| {},
            )
            .unwrap();

        let other_root = TempDir::new().unwrap();
        let other_root = other_root.path().canonicalize().unwrap();
        let store = SessionStore::open(&harness.paths.session_db).unwrap();
        let foreign = store.create(&other_root).unwrap();
        store
            .save(
                &foreign.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "foreign session".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        handle_command(
            &mut stdout,
            &mut state,
            &mut harness.app,
            Command::SessionClear,
        )
        .unwrap();

        assert_eq!(state.messages.len(), 2);
        assert!(state.messages[0].content.contains("ready. Root:"));
        assert_eq!(
            state.messages[1].content,
            "current project sessions cleared; started fresh session"
        );
        assert_eq!(state.status, "ready");
        assert!(state
            .messages
            .iter()
            .all(|m| !m.content.contains("stale user message")));
        assert!(state
            .messages
            .iter()
            .all(|m| !m.content.contains("stale assistant message")));

        let sessions_after_clear = harness.app.list_sessions().unwrap();
        assert_eq!(sessions_after_clear.len(), 1);
        assert_eq!(sessions_after_clear[0].message_count, 0);

        harness
            .app
            .handle(
                RuntimeRequest::Submit {
                    text: "after clear".into(),
                },
                &mut |_| {},
            )
            .unwrap();

        let sessions_after_submit = harness.app.list_sessions().unwrap();
        assert_eq!(sessions_after_submit.len(), 1);
        assert_eq!(sessions_after_submit[0].message_count, 2);
        assert_eq!(
            store
                .list_for_project(other_root.to_string_lossy().as_ref())
                .unwrap()
                .len(),
            1
        );
    }

    struct TestHarness {
        _root_dir: TempDir,
        config: Config,
        paths: AppPaths,
        app: AppContext,
    }

    impl TestHarness {
        fn new() -> Self {
            let root_dir = TempDir::new().unwrap();
            fs::create_dir_all(root_dir.path().join("data")).unwrap();
            fs::create_dir_all(root_dir.path().join("logs")).unwrap();

            let project_root = ProjectRoot::new(root_dir.path().to_path_buf()).unwrap();
            let paths = AppPaths {
                root_dir: root_dir.path().to_path_buf(),
                project_root: root_dir.path().to_path_buf(),
                config_file: root_dir.path().join("config.toml"),
                data_dir: root_dir.path().join("data"),
                logs_dir: root_dir.path().join("logs"),
                session_db: root_dir.path().join("data").join("sessions.db"),
            };
            let config = Config::default();
            let backend = build_backend(&config).unwrap();
            let registry = default_registry().with_project_root(project_root.as_path_buf());
            let (session, history, anchors) =
                ActiveSession::open_or_restore(&paths.session_db, &project_root).unwrap();
            let app = AppContext::build(
                &config,
                project_root,
                backend,
                registry,
                session,
                history,
                anchors,
                None,
                Some(&paths.session_db),
            )
            .unwrap();

            Self {
                _root_dir: root_dir,
                config,
                paths,
                app,
            }
        }
    }

    #[test]
    fn context_usage_event_sets_context_pct() {
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);

        assert_eq!(state.context_pct, None, "starts with no indicator");

        apply_runtime_event(
            &mut state,
            RuntimeEvent::ContextUsage {
                prompt_tokens: 64_000,
                context_window_tokens: 128_000,
            },
        );

        assert_eq!(state.context_pct, Some(50));
    }

    #[test]
    fn context_usage_event_clamps_at_100_pct() {
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);

        apply_runtime_event(
            &mut state,
            RuntimeEvent::ContextUsage {
                prompt_tokens: 200_000,
                context_window_tokens: 128_000,
            },
        );

        assert_eq!(state.context_pct, Some(100));
    }
}
