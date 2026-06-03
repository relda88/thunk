pub(crate) struct CollapsibleSummary {
    pub(crate) summary: String,
    pub(crate) preview_lines: Vec<String>,
}

pub(crate) fn classify_collapsible(content: &str) -> CollapsibleSummary {
    const SINGLE_LINE_PREFIXES: &[&str] = &[
        "tool: ",
        "found ",
        "no matches for '",
        "search: ",
        "read ",
        "read: ",
        "listed ",
        "ls: ",
        "git branch:",
        "git status",
        "git diff",
        "git log",
        "replaced ",
        "created ",
        "overwrote ",
        "shell exit ",
        "shell timed out:",
        "lsp_definition: ",
        "last read:",
        "no anchors set",
        "error: ",
    ];

    for prefix in SINGLE_LINE_PREFIXES {
        if content.starts_with(prefix) {
            return CollapsibleSummary {
                summary: content.to_string(),
                preview_lines: Vec::new(),
            };
        }
    }

    if content.starts_with("diff --git ") {
        let file_count = content
            .lines()
            .filter(|l| l.starts_with("diff --git "))
            .count();
        return CollapsibleSummary {
            summary: format!("git diff: {} file(s) changed", file_count),
            preview_lines: Vec::new(),
        };
    }

    if content.starts_with("history:\n") {
        let preview_lines: Vec<String> = content
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .take(2)
            .map(|l| l.to_string())
            .collect();
        return CollapsibleSummary {
            summary: "conversation history".to_string(),
            preview_lines,
        };
    }

    // Fallback: first line as summary (up to 60 chars), next 2 non-empty lines as preview.
    let mut lines = content.lines();
    let first = lines.next().unwrap_or("");
    let summary: String = first.chars().take(60).collect();
    let preview_lines: Vec<String> = lines
        .filter(|l| !l.trim().is_empty())
        .take(2)
        .map(|l| l.to_string())
        .collect();

    CollapsibleSummary {
        summary,
        preview_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::classify_collapsible;

    #[test]
    fn tool_call_is_single_line() {
        let c = classify_collapsible("tool: read_file");
        assert_eq!(c.summary, "tool: read_file");
        assert!(c.preview_lines.is_empty());
    }

    #[test]
    fn search_result_is_single_line() {
        let c = classify_collapsible("found 3 match(es) for 'foo'");
        assert_eq!(c.summary, "found 3 match(es) for 'foo'");
        assert!(c.preview_lines.is_empty());
    }

    #[test]
    fn history_produces_summary_and_preview() {
        let content = "history:\n[user] hello\n[assistant] world";
        let c = classify_collapsible(content);
        assert_eq!(c.summary, "conversation history");
        assert_eq!(c.preview_lines, vec!["[user] hello", "[assistant] world"]);
    }

    #[test]
    fn fallback_multi_line_extracts_first_line_and_preview() {
        let content = "some unknown output\nline two\nline three\nline four";
        let c = classify_collapsible(content);
        assert_eq!(c.summary, "some unknown output");
        assert_eq!(c.preview_lines, vec!["line two", "line three"]);
    }

    #[test]
    fn fallback_single_line_has_no_preview() {
        let c = classify_collapsible("just one line");
        assert_eq!(c.summary, "just one line");
        assert!(c.preview_lines.is_empty());
    }

    #[test]
    fn fallback_summary_truncates_at_60_chars() {
        let long = "a".repeat(80);
        let c = classify_collapsible(&long);
        assert_eq!(c.summary.chars().count(), 60);
    }

    #[test]
    fn history_skips_empty_lines_in_preview() {
        let content = "history:\n\n[user] hi\n\n[assistant] there";
        let c = classify_collapsible(content);
        assert_eq!(c.summary, "conversation history");
        assert_eq!(c.preview_lines, vec!["[user] hi", "[assistant] there"]);
    }

    #[test]
    fn git_status_summary_is_single_line() {
        let c = classify_collapsible("git status clean on main");
        assert_eq!(c.summary, "git status clean on main");
        assert!(c.preview_lines.is_empty());
    }

    #[test]
    fn no_matches_prefix_is_single_line() {
        let c = classify_collapsible("no matches for 'foo'");
        assert_eq!(c.summary, "no matches for 'foo'");
        assert!(c.preview_lines.is_empty());
    }
}
