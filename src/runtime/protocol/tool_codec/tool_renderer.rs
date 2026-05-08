// Outbound: ToolOutput -> conversation text

use crate::tools::{EntryKind, ToolOutput};

/// Returns a compact one-line summary of a tool result for TUI display.
/// This is separate from format_tool_result, which produces the full conversation text.
pub fn render_compact_summary(output: &ToolOutput) -> String {
    match output {
        ToolOutput::FileContents(f) => {
            if f.truncated {
                format!("read {} ({} lines, truncated)", f.path, f.total_lines)
            } else {
                format!("read {} ({} lines)", f.path, f.total_lines)
            }
        }
        ToolOutput::DirectoryListing(d) => {
            if d.truncated {
                format!(
                    "listed {} (showing {} of {} entries)",
                    d.path,
                    d.entries.len(),
                    d.total_entries
                )
            } else {
                format!("listed {} ({} entries)", d.path, d.entries.len())
            }
        }
        ToolOutput::SearchResults(s) => {
            if s.total_matches == 0 {
                format!("no matches for '{}'", s.query)
            } else if s.truncated {
                format!(
                    "found {} match(es) for '{}' (showing {})",
                    s.total_matches,
                    s.query,
                    s.matches.len()
                )
            } else {
                format!("found {} match(es) for '{}'", s.total_matches, s.query)
            }
        }
        ToolOutput::GitStatus(g) => {
            let branch = g.branch.as_deref().unwrap_or("unknown branch");
            if g.total_entries == 0 {
                format!("git status clean on {branch}")
            } else if g.truncated {
                format!(
                    "git status on {branch}: showing {} of {} entries",
                    g.entries.len(),
                    g.total_entries
                )
            } else if g.total_entries == 1 {
                format!("git status on {branch}: 1 entry")
            } else {
                format!("git status on {branch}: {} entries", g.total_entries)
            }
        }
        ToolOutput::GitDiff(d) => {
            if d.bytes_shown == 0 {
                "git diff empty".to_string()
            } else if d.truncated {
                format!("git diff ({} bytes, truncated)", d.bytes_shown)
            } else {
                format!("git diff ({} bytes)", d.bytes_shown)
            }
        }
        ToolOutput::GitLog(g) => {
            if g.entries.is_empty() {
                "git log empty".to_string()
            } else if g.truncated && g.entries.len() == 1 {
                "git log (1 commit, truncated)".to_string()
            } else if g.truncated {
                format!("git log ({} commits, truncated)", g.entries.len())
            } else if g.entries.len() == 1 {
                "git log (1 commit)".to_string()
            } else {
                format!("git log ({} commits)", g.entries.len())
            }
        }
        ToolOutput::EditFile(e) => {
            format!("replaced {} line(s) in {}", e.lines_replaced, e.path)
        }
        ToolOutput::WriteFile(w) => {
            let verb = if w.created { "created" } else { "overwrote" };
            format!("{} {} ({} bytes)", verb, w.path, w.bytes_written)
        }
    }
}

/// Formats a successful tool result for insertion into the conversation.
/// Uses === delimiters rather than bracket tags so the result format is visually
/// distinct from executable call syntax ([tool_name: arg]).
pub fn format_tool_result(name: &str, output: &ToolOutput) -> String {
    let body = render_output(output);
    format!("=== tool_result: {name} ===\n{body}\n=== /tool_result ===\n\n")
}

/// Like format_tool_result but renders search results with definition candidates sorted first.
/// For DefinitionLookup mode: source-tier files containing definition-like match lines appear
/// before import/usage files, so the model is more likely to read the definition site first.
/// Non-search-result outputs are rendered identically to format_tool_result.
pub fn format_tool_result_definition_ordered(name: &str, output: &ToolOutput) -> String {
    let body = match output {
        ToolOutput::SearchResults(s) => {
            if s.matches.is_empty() {
                "No matches found.".to_string()
            } else {
                render_search_results_grouped(s, true)
            }
        }
        other => render_output(other),
    };
    format!("=== tool_result: {name} ===\n{body}\n=== /tool_result ===\n\n")
}

/// Formats a tool dispatch error for insertion into the conversation.
pub fn format_tool_error(name: &str, error: &str) -> String {
    format!("=== tool_error: {name} ===\n{error}\n=== /tool_error ===\n\n")
}

/// Maximum number of match lines shown per file in grouped search output.
/// Files with more hits show this many lines plus a "(N more not shown)" note.
/// Kept small so a single high-match file cannot crowd out other files in the window.
const MAX_LINES_PER_FILE: usize = 3;

/// Returns true if the file path belongs to the source tier.
/// Mirrors the tier-0 set from search_code::file_class_priority without importing it.
fn is_source_tier(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "rs" | "go"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "h"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "html"
            | "css"
            | "scss"
            | "sql"
            | "xml"
    )
}

/// Returns true if the line (after stripping leading whitespace) looks like a symbol definition.
/// Coverage: Rust, Python, Go, TypeScript, JavaScript.
/// C/C++ patterns are excluded — too many false positives without a type parser.
/// No regex, no scoring — prefix matching only.
pub fn looks_like_definition(line: &str) -> bool {
    let t = line.trim_start();
    // Rust
    t.starts_with("pub enum ")
        || t.starts_with("pub struct ")
        || t.starts_with("pub fn ")
        || t.starts_with("pub type ")
        || t.starts_with("pub trait ")
        || t.starts_with("pub const ")
        || t.starts_with("pub static ")
        || t.starts_with("enum ")
        || t.starts_with("struct ")
        || t.starts_with("fn ")
        || t.starts_with("type ")
        || t.starts_with("const ")
        || t.starts_with("trait ")
        || t.starts_with("impl ")
        // Python / TypeScript / JavaScript (shared keywords)
        || t.starts_with("class ")
        // Python
        || t.starts_with("def ")
        // Go
        || t.starts_with("func ")
        // TypeScript / JavaScript
        || t.starts_with("function ")
        || t.starts_with("interface ")
}

/// Returns true if the line defines exactly `query` as its top-level symbol.
/// Trims leading whitespace, strips the definition keyword prefix,
/// extracts the first identifier after that prefix, and compares it exactly to `query`.
/// Does NOT match type annotations in function parameters.
/// Mirrors the heuristic in `tools::search_code::is_exact_symbol_definition`.
fn is_exact_symbol_definition(line: &str, query: &str) -> bool {
    let line = line.trim_start();
    let def_prefixes = [
        "pub struct ",
        "pub const ",
        "pub static ",
        "pub enum ",
        "pub fn ",
        "pub type ",
        "pub trait ",
        "function ",
        "interface ",
        "struct ",
        "enum ",
        "class ",
        "impl ",
        "const ",
        "trait ",
        "def ",
        "func ",
        "type ",
        "fn ",
    ];
    for prefix in def_prefixes {
        if let Some(after_prefix) = line.strip_prefix(prefix) {
            let first_ident = after_prefix
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next();
            if let Some(ident) = first_ident {
                return ident == query;
            }
            return false;
        }
    }
    false
}

/// Returns the path of the definition-site file for `query`, or None when ambiguous.
///
/// Priority:
///   Tier 1 — exactly one source-tier file where any match line is an exact symbol
///             definition for `query` (e.g. "class Task:" for query "Task").
///   Tier 2 — exactly one source-tier file where all match lines look like definitions
///             (strict fallback for queries that don't trigger exact-match detection).
///
/// Returns None when zero or more than one candidate exists at the winning tier.
fn definition_site_file<'a>(
    groups: &[(&'a str, Vec<&crate::tools::types::SearchMatch>)],
    query: &str,
) -> Option<&'a str> {
    let mut exact_found: Option<&'a str> = None;
    let mut exact_count: usize = 0;
    let mut all_def_found: Option<&'a str> = None;
    let mut all_def_count: usize = 0;

    for (file, matches) in groups {
        if !is_source_tier(file) {
            continue;
        }
        if matches
            .iter()
            .any(|m| is_exact_symbol_definition(&m.line, query))
        {
            exact_count += 1;
            exact_found = Some(file);
        } else if matches.iter().all(|m| looks_like_definition(&m.line)) {
            all_def_count += 1;
            all_def_found = Some(file);
        }
    }

    if exact_count == 1 {
        exact_found
    } else if exact_count == 0 && all_def_count == 1 {
        all_def_found
    } else {
        None
    }
}

fn render_search_results_grouped(
    s: &crate::tools::types::SearchResultsOutput,
    sort_definitions_first: bool,
) -> String {
    use crate::tools::types::SearchMatch;

    let mut lines: Vec<String> = Vec::new();

    if s.truncated {
        lines.push(format!(
            "[showing first {} of {} matches — read a specific matched file with read_file]",
            s.matches.len(),
            s.total_matches
        ));
    }

    // Group consecutive matches that share the same file path.
    // Matches arrive in tier-sorted, then within-tier alphabetical order (Phase 9.0.1),
    // so same-file matches are already adjacent — a single linear pass suffices.
    let mut groups: Vec<(&str, Vec<&SearchMatch>)> = Vec::new();
    for m in &s.matches {
        match groups.last_mut() {
            Some((file, group_matches)) if *file == m.file.as_str() => {
                group_matches.push(m);
            }
            _ => groups.push((m.file.as_str(), vec![m])),
        }
    }

    // For DefinitionLookup: stable-sort groups using a three-level key so the most
    // specific definition file appears first.  Relative order within each tier is
    // preserved (sort_by_key is stable).
    //
    // Key: (!has_exact_def, !has_all_def)
    //   (false, false) — source file where every match is an exact symbol definition
    //   (false, true)  — source file with an exact definition but also usage lines
    //   (true,  false) — source file where all matches are definitions (non-exact)
    //   (true,  true)  — usage-only or non-source file
    if sort_definitions_first {
        let query = &s.query;
        groups.sort_by_key(|(file, matches)| {
            let has_exact_def = is_source_tier(file)
                && matches
                    .iter()
                    .any(|m| is_exact_symbol_definition(&m.line, query));
            let has_all_def =
                is_source_tier(file) && matches.iter().all(|m| looks_like_definition(&m.line));
            (!has_exact_def, !has_all_def)
        });
    }

    // If exactly one source-tier file has a definition-like line, prepend a directive
    // so the model reads the definition site rather than a high-match usage file.
    if let Some(def_file) = definition_site_file(&groups, &s.query) {
        lines.push(format!(
            "[definition found in {} — read this file first]",
            def_file
        ));
    }

    for (file, group_matches) in groups {
        let total_in_file = group_matches.len();
        let shown = total_in_file.min(MAX_LINES_PER_FILE);
        let is_exact_def = s.query.len() > 0
            && group_matches
                .iter()
                .any(|m| is_exact_symbol_definition(&m.line, &s.query));
        let header = if is_exact_def {
            format!("{} [EXACT DEFINITION] ({} matches)", file, total_in_file)
        } else if shown < total_in_file {
            format!("{} ({} matches, showing {})", file, total_in_file, shown)
        } else {
            format!("{} ({} matches)", file, total_in_file)
        };
        lines.push(header);
        for m in &group_matches[..shown] {
            lines.push(format!("  {}: {}", m.line_number, m.line));
        }
    }

    lines.join("\n")
}

fn render_git_status(g: &crate::tools::types::GitStatusOutput) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "branch: {}",
        g.branch.as_deref().unwrap_or("(unknown)")
    ));
    if let Some(upstream) = &g.upstream {
        lines.push(format!("upstream: {upstream}"));
    }
    if let Some(ahead) = g.ahead {
        lines.push(format!("ahead: {ahead}"));
    }
    if let Some(behind) = g.behind {
        lines.push(format!("behind: {behind}"));
    }

    if g.entries.is_empty() {
        lines.push("working tree clean".to_string());
    } else {
        if g.truncated {
            lines.push(format!(
                "[showing first {} of {} status entries]",
                g.entries.len(),
                g.total_entries
            ));
        }
        for entry in &g.entries {
            let suffix = if entry.path_truncated {
                " [path truncated]"
            } else {
                ""
            };
            lines.push(format!("{} {}{}", entry.xy, entry.path, suffix));
        }
    }

    lines.join("\n")
}

fn render_git_diff(d: &crate::tools::types::GitDiffOutput) -> String {
    if d.patch.is_empty() {
        return "No unstaged changes.".to_string();
    }

    if d.truncated {
        format!(
            "[showing first {} bytes of git diff]\n{}\n[truncated]",
            d.bytes_shown, d.patch
        )
    } else {
        d.patch.clone()
    }
}

fn render_git_log(g: &crate::tools::types::GitLogOutput) -> String {
    if g.entries.is_empty() {
        return "No commits found.".to_string();
    }

    let mut lines = Vec::new();
    if g.truncated {
        lines.push(format!(
            "[showing {} recent commits; output truncated]",
            g.entries.len()
        ));
    }
    for entry in &g.entries {
        lines.push(format!(
            "{} {} {} - {}",
            entry.short_hash, entry.date, entry.author, entry.subject
        ));
    }

    lines.join("\n")
}

pub(crate) fn render_output(output: &ToolOutput) -> String {
    match output {
        ToolOutput::FileContents(f) => {
            if f.truncated {
                let shown = f.contents.lines().count();
                let remaining = f.total_lines.saturating_sub(shown);
                format!(
                    "[{total} lines — showing first {shown}]\n{contents}\n[truncated: {remaining} lines not shown]",
                    total = f.total_lines,
                    contents = f.contents,
                )
            } else {
                format!("[{} lines]\n{}", f.total_lines, f.contents)
            }
        }
        ToolOutput::DirectoryListing(d) => {
            if d.entries.is_empty() {
                "(empty directory)".to_string()
            } else {
                let mut lines: Vec<String> = d
                    .entries
                    .iter()
                    .map(|e| {
                        let kind = match e.kind {
                            EntryKind::Dir => "dir ",
                            EntryKind::File => "file",
                            EntryKind::Symlink => "link",
                        };
                        format!("{kind}  {}", e.name)
                    })
                    .collect();
                if d.truncated {
                    let remaining = d.total_entries - d.entries.len();
                    lines.push(format!(
                        "[... {remaining} more entries not shown — {total} total]",
                        total = d.total_entries,
                    ));
                }
                lines.join("\n")
            }
        }
        ToolOutput::SearchResults(s) => {
            if s.matches.is_empty() {
                "No matches found.".to_string()
            } else {
                render_search_results_grouped(s, false)
            }
        }
        ToolOutput::GitStatus(g) => render_git_status(g),
        ToolOutput::GitDiff(d) => render_git_diff(d),
        ToolOutput::GitLog(g) => render_git_log(g),
        ToolOutput::EditFile(e) => {
            format!("replaced {} line(s) in {}", e.lines_replaced, e.path)
        }
        ToolOutput::WriteFile(w) => {
            let verb = if w.created { "created" } else { "overwrote" };
            format!("{} {} ({} bytes)", verb, w.path, w.bytes_written)
        }
    }
}

// Protocol description

/// Returns the format instructions block that prompt.rs includes in the system prompt.
/// Keeping this here ensures the prompt's description always matches the actual
/// formats that the scanners expect and format_tool_result produces.
pub fn format_instructions() -> &'static str {
    r#"TOOL USE RULES — read carefully:

Your role: emit tool CALL TAGS only. The system executes them and returns results.
You do NOT produce file contents, directory listings, or search results yourself.
You do NOT write result blocks. Result blocks are written by the system, not you.

When a tool is needed, your ENTIRE response must be the call tag only — no prose, no fences, no explanation.

Tag names are EXACT. Do not rename, abbreviate, or invent tag names. Use only the tags shown below.

Request a file read:
[read_file: path/to/file.rs]

List a directory:
[list_dir: src/]

Search code:
[search_code: keyword]

Use search_code for any question about where something is, how something works, or what something is called in this project. Do not ask the user — search first. To find code, use search_code directly — do not list directories first.
Use exactly one plain literal keyword or identifier, such as logging, write_file, SessionLog, or sessions. Search using a meaningful keyword from the question (e.g., a function name, variable, or concept) — not a directory name or file path.
Do not use phrases, dots, parentheses, backslashes, regex syntax, or method-call syntax.
Emit only one search call at a time. If the results point to a specific file but do not show enough detail, read that file once with read_file, then respond. Never emit a second search_code.
Only if results are completely empty, try one different single keyword, then stop searching and respond.

Show git working tree status:
[git_status]

Show unstaged git working tree diff:
[git_diff]

Show recent git commit history:
[git_log]

Edit a file:
[edit_file]
path: path/to/file.rs
---search---
old content
---replace---
new content
[/edit_file]

Create an empty file (simple form):
[write_file: path/to/file.rs]

Create or overwrite a file with content:
[write_file]
path: path/to/file.rs
---content---
full file content
[/write_file]

When you have enough information, respond directly in plain text with no tool tags."#
}

#[cfg(test)]
mod tests {
    use super::*;

    // Outbound formatting

    #[test]
    fn format_tool_result_wraps_body() {
        use crate::tools::types::FileContentsOutput;
        use crate::tools::ToolOutput;
        let output = ToolOutput::FileContents(FileContentsOutput {
            path: "x.rs".into(),
            contents: "fn main() {}".into(),
            total_lines: 1,
            truncated: false,
        });
        let result = format_tool_result("read_file", &output);
        assert!(result.starts_with("=== tool_result: read_file ==="));
        assert!(result.contains("[1 lines]"));
        assert!(result.contains("fn main() {}"));
        assert!(result.contains("=== /tool_result ==="));
    }

    #[test]
    fn render_git_status_output() {
        use crate::tools::types::{GitStatusEntry, GitStatusOutput};
        use crate::tools::ToolOutput;

        let output = ToolOutput::GitStatus(GitStatusOutput {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead: Some(1),
            behind: None,
            entries: vec![GitStatusEntry {
                xy: " M".into(),
                path: "src/main.rs".into(),
                path_truncated: false,
            }],
            total_entries: 1,
            truncated: false,
        });

        assert_eq!(
            render_compact_summary(&output),
            "git status on main: 1 entry"
        );
        let rendered = format_tool_result("git_status", &output);
        assert!(rendered.contains("branch: main"));
        assert!(rendered.contains("upstream: origin/main"));
        assert!(rendered.contains("ahead: 1"));
        assert!(rendered.contains(" M src/main.rs"));
    }

    #[test]
    fn render_git_diff_output() {
        use crate::tools::types::GitDiffOutput;
        use crate::tools::ToolOutput;

        let output = ToolOutput::GitDiff(GitDiffOutput {
            patch: "diff --git a/a.rs b/a.rs\n+new\n".into(),
            bytes_shown: 31,
            truncated: true,
        });

        assert_eq!(
            render_compact_summary(&output),
            "git diff (31 bytes, truncated)"
        );
        let rendered = format_tool_result("git_diff", &output);
        assert!(rendered.contains("[showing first 31 bytes of git diff]"));
        assert!(rendered.contains("diff --git a/a.rs b/a.rs"));
        assert!(rendered.contains("[truncated]"));
    }

    #[test]
    fn render_git_log_output() {
        use crate::tools::types::{GitLogEntry, GitLogOutput};
        use crate::tools::ToolOutput;

        let output = ToolOutput::GitLog(GitLogOutput {
            entries: vec![GitLogEntry {
                hash: "0123456789012345678901234567890123456789".into(),
                short_hash: "0123456".into(),
                date: "2026-04-22".into(),
                author: "thunk".into(),
                subject: "add git log".into(),
            }],
            truncated: true,
        });

        assert_eq!(
            render_compact_summary(&output),
            "git log (1 commit, truncated)"
        );
        let rendered = format_tool_result("git_log", &output);
        assert!(rendered.contains("[showing 1 recent commits; output truncated]"));
        assert!(rendered.contains("0123456 2026-04-22 thunk - add git log"));
    }

    #[test]
    fn render_output_includes_metadata_line_for_untruncated_file() {
        use crate::tools::types::FileContentsOutput;
        use crate::tools::ToolOutput;
        let output = ToolOutput::FileContents(FileContentsOutput {
            path: "x.rs".into(),
            contents: "line 1\nline 2".into(),
            total_lines: 2,
            truncated: false,
        });
        let body = render_output(&output);
        assert_eq!(body, "[2 lines]\nline 1\nline 2");
    }

    #[test]
    fn render_output_includes_truncation_notice_for_large_file() {
        use crate::tools::types::FileContentsOutput;
        use crate::tools::ToolOutput;
        // Simulate a 412-line file where only 200 lines are in contents
        let shown_content: String = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = ToolOutput::FileContents(FileContentsOutput {
            path: "big.rs".into(),
            contents: shown_content,
            total_lines: 412,
            truncated: true,
        });
        let body = render_output(&output);
        assert!(
            body.starts_with("[412 lines — showing first 200]"),
            "got: {body}"
        );
        assert!(body.contains("line 0"));
        assert!(
            body.ends_with("[truncated: 212 lines not shown]"),
            "got: {body}"
        );
    }

    #[test]
    fn format_tool_error_wraps_message() {
        let result = format_tool_error("read_file", "file not found");
        assert!(result.starts_with("=== tool_error: read_file ==="));
        assert!(result.contains("file not found"));
        assert!(result.contains("=== /tool_error ==="));
    }

    // SearchResults grouped rendering

    fn make_match(file: &str, line_number: usize, line: &str) -> crate::tools::types::SearchMatch {
        crate::tools::types::SearchMatch {
            file: file.to_string(),
            line_number,
            line: line.to_string(),
        }
    }

    fn make_search_output(
        matches: Vec<crate::tools::types::SearchMatch>,
        total_matches: usize,
    ) -> ToolOutput {
        use crate::tools::types::SearchResultsOutput;
        let truncated = total_matches > matches.len();
        ToolOutput::SearchResults(SearchResultsOutput {
            query: "q".into(),
            matches,
            total_matches,
            truncated,
        })
    }

    #[test]
    fn search_results_grouped_single_file_within_cap() {
        let output = make_search_output(
            vec![
                make_match("src/lib.rs", 10, "fn needle() {}"),
                make_match("src/lib.rs", 20, "let needle = 1;"),
            ],
            2,
        );
        let body = render_output(&output);
        // Header shows file with count, no "showing K" because within cap.
        assert!(
            body.contains("src/lib.rs (2 matches)"),
            "expected file header with count; got:\n{body}"
        );
        assert!(!body.contains("showing"), "no 'showing' annotation needed");
        assert!(body.contains("  10: fn needle() {}"));
        assert!(body.contains("  20: let needle = 1;"));
    }

    #[test]
    fn search_results_grouped_single_file_exceeds_per_file_cap() {
        let matches = (1..=5)
            .map(|i| make_match("src/lib.rs", i, &format!("needle line {i}")))
            .collect();
        let output = make_search_output(matches, 5);
        let body = render_output(&output);
        // 5 matches, cap is MAX_LINES_PER_FILE (3) → header says "showing 3".
        assert!(
            body.contains("src/lib.rs (5 matches, showing 3)"),
            "got:\n{body}"
        );
        assert!(body.contains("  1: needle line 1"));
        assert!(body.contains("  3: needle line 3"));
        assert!(
            !body.contains("  4: needle line 4"),
            "lines beyond cap must not appear"
        );
    }

    #[test]
    fn search_results_grouped_multiple_files_separate_groups() {
        let output = make_search_output(
            vec![
                make_match("src/types.rs", 47, "pub enum TaskStatus {"),
                make_match("src/engine.rs", 312, "let task = Task::new();"),
                make_match("README.md", 12, "A task is a unit"),
            ],
            3,
        );
        let body = render_output(&output);
        assert!(body.contains("src/types.rs (1 matches)"), "got:\n{body}");
        assert!(body.contains("src/engine.rs (1 matches)"), "got:\n{body}");
        assert!(body.contains("README.md (1 matches)"), "got:\n{body}");
        // types.rs must appear before engine.rs, which must appear before README.md.
        let pos_types = body.find("src/types.rs").unwrap();
        let pos_engine = body.find("src/engine.rs").unwrap();
        let pos_readme = body.find("README.md").unwrap();
        assert!(pos_types < pos_engine, "file order must be preserved");
        assert!(pos_engine < pos_readme, "file order must be preserved");
    }

    #[test]
    fn search_results_grouped_truncation_notice_present_when_truncated() {
        let matches: Vec<_> = (1..=3)
            .map(|i| make_match("src/lib.rs", i, "needle"))
            .collect();
        // total_matches = 20 but only 3 are in the shown set → truncated.
        let output = make_search_output(matches, 20);
        let body = render_output(&output);
        assert!(
            body.contains("[showing first 3 of 20 matches"),
            "truncation notice must appear; got:\n{body}"
        );
        assert!(body.contains("src/lib.rs (3 matches)"));
    }

    #[test]
    fn search_results_grouped_no_matches_returns_sentinel() {
        use crate::tools::types::SearchResultsOutput;
        let output = ToolOutput::SearchResults(SearchResultsOutput {
            query: "q".into(),
            matches: vec![],
            total_matches: 0,
            truncated: false,
        });
        let body = render_output(&output);
        assert_eq!(body, "No matches found.");
    }

    #[test]
    fn search_results_grouped_within_file_line_order_preserved() {
        // Lines must appear in ascending line_number order within the file.
        let output = make_search_output(
            vec![
                make_match("src/lib.rs", 5, "first"),
                make_match("src/lib.rs", 12, "second"),
                make_match("src/lib.rs", 99, "third"),
            ],
            3,
        );
        let body = render_output(&output);
        let pos_first = body.find("  5: first").unwrap();
        let pos_second = body.find("  12: second").unwrap();
        let pos_third = body.find("  99: third").unwrap();
        assert!(pos_first < pos_second && pos_second < pos_third);
    }

    // Phase 9.2.1 — Definition Lookup Mode

    #[test]
    fn looks_like_definition_matches_rust_keywords() {
        assert!(looks_like_definition("pub enum TaskStatus {"));
        assert!(looks_like_definition("pub struct Config {"));
        assert!(looks_like_definition("pub fn run_turns("));
        assert!(looks_like_definition("pub type Result<T> ="));
        assert!(looks_like_definition("pub trait Backend {"));
        assert!(looks_like_definition("pub const MAX: usize = 10;"));
        assert!(looks_like_definition("pub static INSTANCE: Lazy<Foo>"));
        assert!(looks_like_definition("enum State {"));
        assert!(looks_like_definition("struct Inner {"));
        assert!(looks_like_definition("fn helper("));
        assert!(looks_like_definition("type Alias = u32;"));
        assert!(looks_like_definition("const CAP: usize = 50;"));
        assert!(looks_like_definition("trait Render {"));
        assert!(looks_like_definition("impl TaskStatus {"));
        // leading whitespace stripped
        assert!(looks_like_definition("    pub fn method("));
        assert!(looks_like_definition("\tfn indented("));
    }

    #[test]
    fn looks_like_definition_matches_other_languages() {
        assert!(looks_like_definition("def my_function(self):"));
        assert!(looks_like_definition("class MyService:"));
        assert!(looks_like_definition("func HandleRequest("));
        assert!(looks_like_definition("function onClick("));
        assert!(looks_like_definition("interface UserRepo {"));
        assert!(looks_like_definition(
            "class Component extends React.Component {"
        ));
        assert!(looks_like_definition("type Config = {"));
        assert!(looks_like_definition("const handler = ("));
    }

    #[test]
    fn looks_like_definition_rejects_usage_lines() {
        assert!(!looks_like_definition("let x = TaskStatus::Running;"));
        assert!(!looks_like_definition("task.execute();"));
        assert!(!looks_like_definition(
            "use crate::tools::types::SearchMatch;"
        ));
        assert!(!looks_like_definition("// pub fn commented_out("));
        assert!(!looks_like_definition("println!(\"fn not a definition\");"));
        assert!(!looks_like_definition("x.fn_call()"));
        assert!(!looks_like_definition("result = some_fn(a, b)"));
    }

    #[test]
    fn definition_preamble_fires_for_only_source_file() {
        // Exactly one source-tier file in the results and it has a definition line.
        // No other source files present — preamble must fire.
        let output = make_search_output(
            vec![
                make_match("src/types.rs", 47, "pub enum TaskStatus {"),
                make_match("README.md", 3, "TaskStatus represents the task state."),
            ],
            2,
        );
        let body = render_output(&output);
        assert!(
            body.contains("[definition found in src/types.rs — read this file first]"),
            "preamble must fire when types.rs is the only source-tier file; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_fires_for_single_definition_file_with_usage_files() {
        // One source file has a definition line; another source file has only usage lines.
        // The runtime usage-evidence gate prevents this hint from admitting definition-only
        // evidence for usage lookups, but definition/location lookups still benefit from it.
        let output = make_search_output(
            vec![
                make_match("src/types.rs", 47, "pub enum TaskStatus {"),
                make_match("src/engine.rs", 312, "let status = TaskStatus::Running;"),
                make_match("src/engine.rs", 415, "task.set_status(status);"),
            ],
            3,
        );
        let body = render_output(&output);
        assert!(
            body.contains("[definition found in src/types.rs — read this file first]"),
            "preamble must fire for the single definition file; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_suppressed_when_multiple_definition_files() {
        // Two source files both have definition lines → ambiguous → no preamble.
        let output = make_search_output(
            vec![
                make_match("src/types.rs", 10, "pub enum TaskStatus {"),
                make_match("src/models.rs", 5, "pub struct Task {"),
            ],
            2,
        );
        let body = render_output(&output);
        assert!(
            !body.contains("[definition found in"),
            "preamble must be suppressed when multiple files have definitions; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_suppressed_when_no_definition_lines() {
        // All match lines are usage sites — no definition patterns → no preamble.
        let output = make_search_output(
            vec![
                make_match("src/engine.rs", 100, "task.execute();"),
                make_match("src/engine.rs", 200, "let t = Task::new();"),
            ],
            2,
        );
        let body = render_output(&output);
        assert!(
            !body.contains("[definition found in"),
            "preamble must not fire when no definition lines exist; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_suppressed_for_docs_tier_only() {
        // README.md mentions a class definition in prose — docs tier, not source tier.
        // Preamble must not fire.
        let output = make_search_output(
            vec![make_match(
                "README.md",
                5,
                "class MyService handles all requests",
            )],
            1,
        );
        let body = render_output(&output);
        assert!(
            !body.contains("[definition found in"),
            "preamble must not fire for docs-tier files; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_correct_when_definition_and_usage_in_source_tier() {
        // types.rs: definition line. engine.rs: usage lines only. Both source tier.
        // Preamble must name types.rs, not engine.rs.
        let output = make_search_output(
            vec![
                make_match("src/types.rs", 47, "pub struct Config {"),
                make_match("src/engine.rs", 88, "let c = Config::default();"),
                make_match("src/engine.rs", 200, "config.apply();"),
                make_match("README.md", 3, "Config is the main configuration type."),
            ],
            4,
        );
        let body = render_output(&output);
        assert!(
            body.contains("[definition found in src/types.rs — read this file first]"),
            "preamble must name the definition file, not usage files; got:\n{body}"
        );
        assert!(!body.contains("src/engine.rs — read this file first"));
    }

    #[test]
    fn definition_preamble_present_with_truncated_results() {
        // total_matches > shown → truncation notice fires.
        // If the shown set includes a single definition file, preamble must still fire.
        let matches = vec![
            make_match("src/types.rs", 10, "pub fn important()"),
            make_match("src/engine.rs", 50, "important();"),
            make_match("src/engine.rs", 60, "important();"),
        ];
        let output = make_search_output(matches, 50); // total=50, shown=3 → truncated
        let body = render_output(&output);
        assert!(
            body.contains("[showing first 3 of 50 matches"),
            "truncation notice must be present"
        );
        assert!(
            body.contains("[definition found in src/types.rs — read this file first]"),
            "preamble must fire even when results are truncated; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_fires_with_truncated_results_and_only_source_file() {
        // total_matches > shown → truncation. Exactly one source-tier file in shown set.
        // Preamble must still fire.
        let matches = vec![
            make_match("src/types.rs", 10, "pub fn important()"),
            make_match("README.md", 20, "important is a key function"),
        ];
        let output = make_search_output(matches, 50); // total=50, shown=2 → truncated
        let body = render_output(&output);
        assert!(
            body.contains("[showing first 2 of 50 matches"),
            "truncation notice must be present"
        );
        assert!(
            body.contains("[definition found in src/types.rs — read this file first]"),
            "preamble must fire when types.rs is the only source-tier file; got:\n{body}"
        );
    }

    #[test]
    fn definition_preamble_does_not_alter_group_rendering() {
        // The preamble is additive — existing group headers and match lines must still appear.
        let output = make_search_output(
            vec![
                make_match("src/types.rs", 47, "pub enum Status {"),
                make_match("src/engine.rs", 10, "status.run();"),
            ],
            2,
        );
        let body = render_output(&output);
        assert!(
            body.contains("src/types.rs (1 matches)"),
            "group header must still appear"
        );
        assert!(
            body.contains("src/engine.rs (1 matches)"),
            "group header must still appear"
        );
        assert!(
            body.contains("  47: pub enum Status {"),
            "match line must still appear"
        );
        assert!(
            body.contains("  10: status.run();"),
            "match line must still appear"
        );
    }

    #[test]
    fn format_instructions_mentions_all_formats() {
        let instructions = format_instructions();
        assert!(instructions.contains("[read_file:"));
        assert!(instructions.contains("[list_dir:"));
        assert!(instructions.contains("[search_code:"));
        assert!(instructions.contains("[git_status]"));
        assert!(instructions.contains("[git_diff]"));
        assert!(instructions.contains("[git_log]"));
        assert!(instructions.contains("[edit_file]"));
        assert!(instructions.contains("[/edit_file]"));
        assert!(instructions.contains("[write_file:"));
        assert!(instructions.contains("[write_file]"));
        assert!(instructions.contains("[/write_file]"));
        assert!(instructions.contains("---search---"));
        assert!(instructions.contains("---replace---"));
        assert!(instructions.contains("---content---"));
    }

    #[test]
    fn format_instructions_does_not_describe_tool_result_format() {
        // Result/error wrappers are runtime-only. Describing their format causes the model
        // to fabricate completed exchanges instead of issuing real calls.
        let instructions = format_instructions();
        assert!(
            !instructions.contains("Tool results are returned as"),
            "must not document the result format for the model"
        );
        assert!(
            !instructions.contains("[tool_result:"),
            "must not show old bracket tool_result syntax in instructions"
        );
        assert!(
            !instructions.contains("[tool_error:"),
            "must not show old bracket tool_error syntax in instructions"
        );
        assert!(
            !instructions.contains("=== tool_result:"),
            "must not show result wrapper format in instructions — model must not learn to fabricate it"
        );
        assert!(
            !instructions.contains("=== tool_error:"),
            "must not show error wrapper format in instructions"
        );
        // Role framing must be present.
        assert!(
            instructions.contains("You do NOT produce"),
            "must include explicit role framing"
        );
    }

    #[test]
    fn format_instructions_contains_exact_tag_warning() {
        let instructions = format_instructions();
        assert!(
            instructions.contains("Tag names are EXACT"),
            "must warn the model about exact tag names"
        );
    }

    // Definition-ordered rendering (Slice 12.1.2)

    #[test]
    fn definition_ordered_puts_definition_file_before_import_file() {
        // Definition file listed second in raw search order → must appear first after ordering
        let output = make_search_output(
            vec![
                make_match(
                    "sandbox/cli/commands.py",
                    10,
                    "from models.enums import TaskStatus",
                ),
                make_match("sandbox/models/enums.py", 5, "class TaskStatus(str, Enum):"),
            ],
            2,
        );
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        let pos_enums = body.find("sandbox/models/enums.py").unwrap();
        let pos_commands = body.find("sandbox/cli/commands.py").unwrap();
        assert!(
            pos_enums < pos_commands,
            "definition file must appear before import file; got:\n{body}"
        );
    }

    #[test]
    fn definition_ordered_preserves_relative_order_among_definition_files() {
        // Two definition files: a.py before b.py in search order → that order is preserved
        let output = make_search_output(
            vec![
                make_match("sandbox/models/a.py", 1, "class TypeA:"),
                make_match("sandbox/models/b.py", 1, "class TypeB:"),
                make_match("sandbox/cli/commands.py", 10, "from models.a import TypeA"),
            ],
            3,
        );
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        let pos_a = body.find("sandbox/models/a.py").unwrap();
        let pos_b = body.find("sandbox/models/b.py").unwrap();
        let pos_cmd = body.find("sandbox/cli/commands.py").unwrap();
        assert!(
            pos_a < pos_b,
            "a.py must remain before b.py among definition files"
        );
        assert!(
            pos_b < pos_cmd,
            "definition files must precede non-definition files"
        );
    }

    #[test]
    fn definition_ordered_preserves_relative_order_among_non_definition_files() {
        // Two non-definition files: import.py before usage.py → preserved after sort
        let output = make_search_output(
            vec![
                make_match("sandbox/models/enums.py", 1, "class TaskStatus(str, Enum):"),
                make_match(
                    "sandbox/cli/import.py",
                    10,
                    "from models.enums import TaskStatus",
                ),
                make_match("sandbox/cli/usage.py", 20, "status = TaskStatus.DONE"),
            ],
            3,
        );
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        let pos_import = body.find("sandbox/cli/import.py").unwrap();
        let pos_usage = body.find("sandbox/cli/usage.py").unwrap();
        assert!(
            pos_import < pos_usage,
            "import.py must remain before usage.py among non-definition files"
        );
    }

    #[test]
    fn definition_ordered_false_matches_render_output_exactly() {
        // sort_definitions_first=false must be bit-for-bit identical to render_output
        let output = make_search_output(
            vec![
                make_match(
                    "sandbox/cli/commands.py",
                    10,
                    "from models.enums import TaskStatus",
                ),
                make_match("sandbox/models/enums.py", 5, "class TaskStatus(str, Enum):"),
            ],
            2,
        );
        let unordered_body = render_output(&output);
        let ToolOutput::SearchResults(ref s) = output else {
            panic!()
        };
        let also_unordered = render_search_results_grouped(s, false);
        assert_eq!(
            unordered_body, also_unordered,
            "sort_definitions_first=false must match render_output exactly"
        );
    }

    #[test]
    fn definition_ordered_groups_appear_before_imports_when_enabled() {
        // Verify that definition file groups appear before non-definition groups in rendered output
        // using two pure usage files (no preamble interference from definition_site_file).
        let output = make_search_output(
            vec![
                make_match("sandbox/cli/commands.py", 10, "import TaskStatus"),
                make_match("sandbox/cli/runners.py", 20, "let status = new_status();"),
                make_match("sandbox/models/enums.py", 1, "class TaskStatus(str, Enum):"),
            ],
            3,
        );
        // With sort: enums.py (definition) must appear before both usage files
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        // Find positions of the file group headers (not preamble)
        // The group header format is "path (N matches)" — find "enums.py (" to avoid preamble hit
        let pos_enums_group = body.find("sandbox/models/enums.py (").unwrap();
        let pos_commands_group = body.find("sandbox/cli/commands.py (").unwrap();
        let pos_runners_group = body.find("sandbox/cli/runners.py (").unwrap();
        assert!(
            pos_enums_group < pos_commands_group,
            "enums.py group must appear before commands.py group"
        );
        assert!(
            pos_enums_group < pos_runners_group,
            "enums.py group must appear before runners.py group"
        );
    }

    #[test]
    fn definition_ordered_non_source_tier_file_not_promoted() {
        // README.md contains prose with "class" in it — docs tier, must not be promoted
        let output = make_search_output(
            vec![
                make_match("README.md", 5, "class TaskStatus handles task state"),
                make_match(
                    "sandbox/cli/commands.py",
                    10,
                    "from models.enums import TaskStatus",
                ),
                make_match("sandbox/models/enums.py", 1, "class TaskStatus(str, Enum):"),
            ],
            3,
        );
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        let pos_readme = body.find("README.md").unwrap();
        let pos_enums = body.find("sandbox/models/enums.py").unwrap();
        // enums.py (source-tier definition) must appear before README.md (docs-tier)
        assert!(
            pos_enums < pos_readme,
            "source-tier definition file must appear before docs-tier file; got:\n{body}"
        );
    }

    #[test]
    fn is_exact_symbol_definition_matches_and_rejects() {
        assert!(is_exact_symbol_definition("class Task:", "Task"));
        assert!(is_exact_symbol_definition("class Task(Base):", "Task"));
        assert!(is_exact_symbol_definition("pub struct Task {", "Task"));
        assert!(is_exact_symbol_definition("fn Task(", "Task"));
        // prefix symbols must not match
        assert!(!is_exact_symbol_definition("class TaskStatus:", "Task"));
        assert!(!is_exact_symbol_definition("struct TaskRunner {", "Task"));
        // non-definition lines must not match even if query is present
        assert!(!is_exact_symbol_definition("x = Task()", "Task"));
    }

    #[test]
    fn definition_ordered_exact_def_beats_prefix_def() {
        // enums.py defines "TaskStatus" (prefix of "Task"), task.py defines "Task" exactly.
        // With query "Task", task.py must appear first.
        let output = ToolOutput::SearchResults(crate::tools::types::SearchResultsOutput {
            query: "Task".into(),
            matches: vec![
                make_match("sandbox/models/enums.py", 5, "class TaskStatus(str, Enum):"),
                make_match("sandbox/models/task.py", 1, "class Task:"),
            ],
            total_matches: 2,
            truncated: false,
        });
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        let pos_task = body.find("sandbox/models/task.py").unwrap();
        let pos_enums = body.find("sandbox/models/enums.py").unwrap();
        assert!(
            pos_task < pos_enums,
            "exact definition file must appear before prefix-definition file; got:\n{body}"
        );
    }

    #[test]
    fn definition_site_file_prefers_exact_over_all_def() {
        // When one file has an exact symbol definition and another has all-def lines,
        // definition_site_file must return the exact-def file (not None).
        let query = "Task";
        let task_match = make_match("sandbox/models/task.py", 1, "class Task:");
        let enums_match = make_match("sandbox/models/enums.py", 5, "class TaskStatus(str, Enum):");
        let groups: Vec<(&str, Vec<&crate::tools::types::SearchMatch>)> = vec![
            ("sandbox/models/enums.py", vec![&enums_match]),
            ("sandbox/models/task.py", vec![&task_match]),
        ];
        let result = definition_site_file(&groups, query);
        assert_eq!(
            result,
            Some("sandbox/models/task.py"),
            "exact-def file must win over all-def prefix file"
        );
    }

    #[test]
    fn definition_ordered_all_usage_files_no_reordering() {
        // All files are usage/import files — no definitions present — order unchanged
        let output = make_search_output(
            vec![
                make_match(
                    "sandbox/cli/commands.py",
                    10,
                    "from models.enums import TaskStatus",
                ),
                make_match("sandbox/services/task.py", 20, "status = TaskStatus.DONE"),
            ],
            2,
        );
        let unordered_body = render_output(&output);
        let body = {
            let ToolOutput::SearchResults(ref s) = output else {
                panic!()
            };
            render_search_results_grouped(s, true)
        };
        // When no definition files exist, sort changes nothing
        assert_eq!(
            unordered_body, body,
            "output must be identical when no definition files are present"
        );
    }
}
