use crate::storage::session::SessionMeta;

pub(super) fn summarize_command_output(text: &str) -> String {
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
        "git_status" | "git_diff" | "git_log" | "web_fetch" => body.to_string(),
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

fn parse_read_file_header(line: &str) -> Option<(usize, bool)> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let truncated = inner.contains(" — ");
    let count_str = inner.split(" — ").next()?.split_whitespace().next()?;
    let n: usize = count_str.parse().ok()?;
    Some((n, truncated))
}

pub(super) fn format_sessions_list(sessions: &[SessionMeta]) -> String {
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

#[cfg(test)]
mod tests {
    use super::{
        format_session_updated_at, format_sessions_list, parse_read_file_header,
        summarize_command_output,
    };

    fn tool_result(name: &str, body: &str) -> String {
        format!("=== tool_result: {name} ===\n{body}\n=== /tool_result ===\n\n")
    }

    // parse_read_file_header

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
}
