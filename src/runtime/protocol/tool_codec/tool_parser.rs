use std::collections::HashMap;

use crate::tools::ToolInput;

// Outer tags for multi-line block tools
const WRITE_OPEN: &str = "[write_file]";
const WRITE_CLOSE: &str = "[/write_file]";
const EDIT_OPEN: &str = "[edit_file]";
const EDIT_CLOSE: &str = "[/edit_file]";
const SEARCH_CODE_OPEN: &str = "[search_code]";
const SEARCH_CODE_CLOSE: &str = "[/search_code]";

const SEARCH_DELIM: &str = "---search---";
const REPLACE_DELIM: &str = "---replace---";
const CONTENT_DELIM: &str = "---content---";
const OLD_CONTENT_LABEL: &str = "old content:";
const NEW_CONTENT_LABEL: &str = "new content:";
// Line-anchored form: require delimiter to appear at the start of a line
// so occurrences embedded mid-line in content are not mistaken for delimiters.
const REPLACE_LINE: &str = "\n---replace---";

// Inbound: model text -> ToolInput

/// Scans model output for all tool call types and returns typed ToolInput values
/// in document order. Malformed or unrecognized blocks are silently skipped.
/// Tool syntax found inside markdown code fences (``` ... ```) is excluded — those
/// are illustrative examples, not real invocations.
pub fn parse_all_tool_inputs(text: &str) -> Vec<ToolInput> {
    let fences = code_fence_ranges(text);
    let mut all: Vec<(usize, ToolInput)> = Vec::new();
    all.extend(scan_bracket_calls(text));
    all.extend(scan_static_bracket_calls(text));
    all.extend(scan_edit_blocks(text));
    all.extend(scan_write_blocks(text));
    all.extend(scan_search_code_blocks(text));
    if !fences.is_empty() {
        all.retain(|(pos, _)| !fences.iter().any(|&(s, e)| *pos >= s && *pos < e));
    }
    all.sort_by_key(|(pos, _)| *pos);
    all.into_iter().map(|(_, input)| input).collect()
}

/// Returns the byte ranges (start, exclusive end) of markdown code fence blocks (``` ... ```).
/// Used to exclude tool syntax inside fences from being treated as real invocations.
fn code_fence_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let Some(rel) = text[pos..].find("```") else {
            break;
        };
        let open = pos + rel;
        let after_marker = open + 3;
        // Skip the optional language tag on the opening fence line (e.g. ```rust)
        let content_start = text[after_marker..]
            .find('\n')
            .map(|r| after_marker + r + 1)
            .unwrap_or(text.len());
        // Find the closing ``` — take the first one after content_start
        let Some(close_rel) = text[content_start..].find("```") else {
            break;
        };
        let close_end = content_start + close_rel + 3;
        ranges.push((open, close_end));
        pos = close_end;
    }
    ranges
}

/// Scans for single-line bracket calls: [read_file: path], [list_dir: path],
/// [search_code: query], [write_file: path], [shell: cargo check].
/// The closing ] must appear on the same line as the opening [.
/// Note: [write_file: path] creates an empty file. Files with content use the block form.
fn scan_bracket_calls(text: &str) -> Vec<(usize, ToolInput)> {
    let mut results = Vec::new();
    let named_tools: &[(&str, &str)] = &[
        ("read_file", "[read_file:"),
        ("list_dir", "[list_dir:"),
        ("search_code", "[search_code:"),
        ("write_file", "[write_file:"),
        ("shell", "[shell:"),
    ];

    for (tool_name, prefix) in named_tools {
        let mut search_start = 0;
        while search_start < text.len() {
            let Some(rel) = text[search_start..].find(prefix) else {
                break;
            };
            let open_abs = search_start + rel;
            let after_colon = open_abs + prefix.len();

            let Some(bracket_rel) = text[after_colon..].find(']') else {
                break;
            };
            let bracket_abs = after_colon + bracket_rel;

            let arg_text = &text[after_colon..bracket_abs];
            // Reject if a newline appears before ]
            if arg_text.contains('\n') {
                search_start = after_colon;
                continue;
            }

            let arg = arg_text.trim();
            if let Some(input) = make_bracket_input(tool_name, arg) {
                results.push((open_abs, input));
            }
            search_start = bracket_abs + 1;
        }
    }

    results
}

fn scan_static_bracket_calls(text: &str) -> Vec<(usize, ToolInput)> {
    let mut results = Vec::new();
    let static_tools: &[(&str, ToolInput)] = &[
        ("[git_status]", ToolInput::GitStatus),
        ("[git_diff]", ToolInput::GitDiff),
        ("[git_log]", ToolInput::GitLog),
    ];

    for (tag, input) in static_tools {
        let mut search_start = 0;
        while search_start < text.len() {
            let Some(rel) = text[search_start..].find(tag) else {
                break;
            };
            let open_abs = search_start + rel;
            results.push((open_abs, input.clone()));
            search_start = open_abs + tag.len();
        }
    }
    results
}

fn make_bracket_input(tool_name: &str, arg: &str) -> Option<ToolInput> {
    match tool_name {
        "read_file" if !arg.is_empty() => Some(ToolInput::ReadFile {
            path: arg.to_string(),
        }),
        "list_dir" => Some(ToolInput::ListDir {
            path: if arg.is_empty() {
                ".".to_string()
            } else {
                arg.to_string()
            },
        }),
        "search_code" if !arg.is_empty() => Some(ToolInput::SearchCode {
            query: arg.to_string(),
            path: None,
        }),
        "write_file" if !arg.is_empty() => {
            let path = arg.strip_prefix("path=").unwrap_or(arg).trim().to_string();
            if path.is_empty() {
                return None;
            }
            Some(ToolInput::WriteFile {
                path,
                content: String::new(),
            })
        }
        "shell" if !arg.is_empty() => Some(ToolInput::Shell {
            command: arg.to_string(),
        }),
        _ => None,
    }
}

fn scan_edit_blocks(text: &str) -> Vec<(usize, ToolInput)> {
    let mut results = Vec::new();
    let mut remaining = text;
    let mut offset = 0usize;

    while let Some(open_pos) = remaining.find(EDIT_OPEN) {
        let after_open = &remaining[open_pos + EDIT_OPEN.len()..];
        match after_open.find(EDIT_CLOSE) {
            Some(close_pos) => {
                let block = &after_open[..close_pos];
                if let Some(input) = parse_edit_block(block) {
                    results.push((offset + open_pos, input));
                }
                let advance = open_pos + EDIT_OPEN.len() + close_pos + EDIT_CLOSE.len();
                offset += advance;
                remaining = &remaining[advance..];
            }
            None => break,
        }
    }

    results
}

fn scan_write_blocks(text: &str) -> Vec<(usize, ToolInput)> {
    let mut results = Vec::new();
    let mut remaining = text;
    let mut offset = 0usize;

    while let Some(open_pos) = remaining.find(WRITE_OPEN) {
        let after_open = &remaining[open_pos + WRITE_OPEN.len()..];
        match after_open.find(WRITE_CLOSE) {
            Some(close_pos) => {
                let block = &after_open[..close_pos];
                if let Some(input) = parse_write_block(block) {
                    results.push((offset + open_pos, input));
                }
                let advance = open_pos + WRITE_OPEN.len() + close_pos + WRITE_CLOSE.len();
                offset += advance;
                remaining = &remaining[advance..];
            }
            None => break,
        }
    }

    results
}

/// Handles the block form `[search_code]\n...\n[/search_code]` that the model
/// sometimes emits when following the edit/write block pattern.
/// Extracts the query from `pattern=X`, `query=X`, or the first non-empty line.
fn scan_search_code_blocks(text: &str) -> Vec<(usize, ToolInput)> {
    let mut results = Vec::new();
    let mut remaining = text;
    let mut offset = 0usize;

    while let Some(open_pos) = remaining.find(SEARCH_CODE_OPEN) {
        let after_open = &remaining[open_pos + SEARCH_CODE_OPEN.len()..];
        match after_open.find(SEARCH_CODE_CLOSE) {
            Some(close_pos) => {
                let block = &after_open[..close_pos];
                if let Some(input) = parse_search_code_block(block) {
                    results.push((offset + open_pos, input));
                }
                let advance =
                    open_pos + SEARCH_CODE_OPEN.len() + close_pos + SEARCH_CODE_CLOSE.len();
                offset += advance;
                remaining = &remaining[advance..];
            }
            None => break,
        }
    }

    results
}

fn parse_search_code_block(block: &str) -> Option<ToolInput> {
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Accept `pattern=X`, `pattern: X`, `query=X`, `query: X`, or bare text.
        // Models commonly emit the colon-space form (matching kv-style formatting),
        // so both separators are tolerated.
        let query = if let Some(rest) = line.strip_prefix("pattern=") {
            rest.trim()
        } else if let Some(rest) = line.strip_prefix("pattern:") {
            rest.trim()
        } else if let Some(rest) = line.strip_prefix("query=") {
            rest.trim()
        } else if let Some(rest) = line.strip_prefix("query:") {
            rest.trim()
        } else {
            line
        };
        if !query.is_empty() {
            return Some(ToolInput::SearchCode {
                query: query.to_string(),
                path: None,
            });
        }
    }
    None
}

fn parse_edit_block(block: &str) -> Option<ToolInput> {
    if let Some(search_pos) = block.find(SEARCH_DELIM) {
        // Full form: both ---search--- and ---replace--- present.
        let after_search = &block[search_pos + SEARCH_DELIM.len()..];
        // Use the line-anchored form so ---replace--- embedded mid-line in the search
        // content (e.g. inside a comment) is not mistaken for the actual delimiter.
        let replace_nl_offset = after_search.find(REPLACE_LINE)?;
        let replace_pos = search_pos + SEARCH_DELIM.len() + replace_nl_offset + 1;

        let path = parse_kvs(&block[..search_pos]).get("path")?.clone();
        let search = trim_block_content(&after_search[..replace_nl_offset]);
        let replace = trim_block_content(&block[replace_pos + REPLACE_DELIM.len()..]);

        Some(ToolInput::EditFile {
            path,
            search,
            replace,
        })
    } else if let Some(replace_nl_pos) = block.find(REPLACE_LINE) {
        // Partial form: ---replace--- present but ---search--- absent.
        // Parse what we can and produce an empty search string. The empty-search
        // validation in edit_file.run() will surface a clear error into the conversation
        // rather than silently discarding the block as a non-tool-call.
        let path = parse_kvs(&block[..replace_nl_pos]).get("path")?.clone();
        let replace = trim_block_content(&block[replace_nl_pos + REPLACE_LINE.len()..]);
        Some(ToolInput::EditFile {
            path,
            search: String::new(),
            replace,
        })
    } else if let Some(input) = parse_edit_block_conflict_style(block) {
        // <<<<<<< SEARCH / ======= / >>>>>>> REPLACE (Aider/git conflict style)
        Some(input)
    } else if let Some(input) = parse_edit_block_labeled_content(block) {
        // old content: ... / new content: ... (observed local-model drift)
        Some(input)
    } else {
        // Generic fallback: any ---xxx--- / ---yyy--- delimiter pair.
        // Models sometimes derive delimiter names from the prompt's placeholder text
        // (e.g. ---text to find--- / ---replacement text---). Accept any valid
        // ---word(s)--- pair rather than silently falling through as a Direct response.
        parse_edit_block_generic_delimiters(block)
    }
}

/// Parses the conflict-marker style that many models emit instead of ---search---/---replace---:
///
///   <<<<<<< SEARCH
///   text to find
///   =======
///   replacement text
///   >>>>>>> REPLACE
fn parse_edit_block_conflict_style(block: &str) -> Option<ToolInput> {
    let search_marker = block.find("<<<<<<<")?;
    let path = parse_kvs(&block[..search_marker]).get("path")?.clone();

    // Skip the rest of the <<<<<<< ... opening line to reach content
    let after_marker = &block[search_marker + "<<<<<<<".len()..];
    let content_start = after_marker
        .find('\n')
        .map(|p| &after_marker[p + 1..])
        .unwrap_or(after_marker);

    // ======= separator must appear at the start of a line
    let sep_pos = content_start.find("\n=======")?;
    let search_text = trim_block_content(&content_start[..sep_pos]);

    let after_sep = &content_start[sep_pos + "\n=======".len()..];
    let after_sep = after_sep.strip_prefix('\n').unwrap_or(after_sep);

    // >>>>>>> end marker — stop before it; trailing text after >>>>>>> is ignored
    let replace_end = after_sep.find("\n>>>>>>>").unwrap_or(after_sep.len());
    let replace_text = trim_block_content(&after_sep[..replace_end]);

    Some(ToolInput::EditFile {
        path,
        search: search_text,
        replace: replace_text,
    })
}

/// Parses the narrow label style observed from local models:
///
///   old content: text to find
///   new content: replacement text
///
/// This is intentionally scoped to `edit_file` and these exact labels. It is not a
/// general key/value edit parser.
fn parse_edit_block_labeled_content(block: &str) -> Option<ToolInput> {
    let (old_line_start, old_value_start) = find_label_line(block, OLD_CONTENT_LABEL, 0)?;
    let (new_line_start, new_value_start) =
        find_label_line(block, NEW_CONTENT_LABEL, old_value_start)?;
    let path = parse_kvs(&block[..old_line_start]).get("path")?.clone();
    let search_text = trim_labeled_content(&block[old_value_start..new_line_start]);
    let replace_text = trim_labeled_content(&block[new_value_start..]);
    Some(ToolInput::EditFile {
        path,
        search: search_text,
        replace: replace_text,
    })
}

fn find_label_line(block: &str, label: &str, start_at: usize) -> Option<(usize, usize)> {
    let mut pos = 0usize;
    for raw_line in block.split_inclusive('\n') {
        if pos < start_at {
            pos += raw_line.len();
            continue;
        }

        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let trimmed = line.trim_start();
        let leading = line.len() - trimmed.len();
        if trimmed.starts_with(label) {
            return Some((pos, pos + leading + label.len()));
        }
        pos += raw_line.len();
    }
    None
}

fn trim_labeled_content(s: &str) -> String {
    let s = s.trim_start_matches(|c| c == ' ' || c == '\t');
    trim_block_content(s)
}

/// Returns true for lines of the form `---word(s)---` that are not the canonical
/// `---search---`, `---replace---`, or `---content---` delimiters (those are handled
/// by the primary branches of `parse_edit_block`). The inner text must be non-empty
/// and must not itself contain `---`, which would indicate a nested or malformed marker.
fn is_triple_dash_delimiter(line: &str) -> bool {
    if !line.starts_with("---") || !line.ends_with("---") || line.len() <= 6 {
        return false;
    }
    let inner = &line[3..line.len() - 3];
    !inner.trim().is_empty() && !inner.contains("---")
}

/// Fallback parser for edit blocks that use arbitrary `---xxx---` / `---yyy---` delimiters.
///
/// Models sometimes derive delimiter names from the prompt's placeholder text rather than
/// using the canonical `---search---`/`---replace---` markers exactly as shown. For example,
/// a model might emit `---text to find---` / `---replacement text---` after reading the
/// `exact text to find` / `replacement text` examples in the instructions. This function
/// accepts any valid `---word(s)---` pair as search/replace delimiters so those blocks
/// are not silently dropped as Direct responses.
fn parse_edit_block_generic_delimiters(block: &str) -> Option<ToolInput> {
    // Collect (line_start, line_end_excl_newline) for each triple-dash delimiter line.
    let mut delimiters: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    for line in block.split('\n') {
        if is_triple_dash_delimiter(line.trim()) {
            delimiters.push((pos, pos + line.len()));
        }
        pos += line.len() + 1; // +1 for the '\n' consumed by split
    }
    if delimiters.len() < 2 {
        return None;
    }
    let (d1_start, d1_end) = delimiters[0];
    let (d2_start, d2_end) = delimiters[1];
    let path = parse_kvs(&block[..d1_start]).get("path")?.clone();
    let search_start = (d1_end + 1).min(block.len());
    let search_text = trim_block_content(&block[search_start..d2_start]);
    let replace_start = (d2_end + 1).min(block.len());
    let replace_text = trim_block_content(&block[replace_start..]);
    Some(ToolInput::EditFile {
        path,
        search: search_text,
        replace: replace_text,
    })
}

fn parse_write_block(block: &str) -> Option<ToolInput> {
    let content_pos = block.find(CONTENT_DELIM)?;

    let path = parse_kvs(&block[..content_pos]).get("path")?.clone();
    let content = trim_block_content(&block[content_pos + CONTENT_DELIM.len()..]);

    Some(ToolInput::WriteFile { path, content })
}

/// Strips exactly one leading newline and one trailing newline from block content.
/// This removes the newlines that immediately follow a delimiter line and precede
/// the next delimiter or closing tag, without touching internal whitespace.
fn trim_block_content(s: &str) -> String {
    let s = s.strip_prefix('\n').unwrap_or(s);
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.to_string()
}

/// Parses `key: value` lines into a map. The first `:` on each line is the separator;
/// values may contain further colons. Whitespace around key and value is trimmed.
fn parse_kvs(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim();
            let value = line[colon + 1..].trim();
            if !key.is_empty() {
                map.insert(key.to_string(), value.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    // Code fence filtering

    #[test]
    fn tool_call_inside_code_fence_is_not_executed() {
        // Model reproduces protocol syntax inside a code fence as an example.
        // Must not be treated as a real invocation.
        let text = "Here is how you use it:\n```\n[write_file: path/to/file.rs]\n```\nThat creates a file.";
        let calls = parse_all_tool_inputs(text);
        assert!(
            calls.is_empty(),
            "tool syntax inside code fence must not execute: {calls:?}"
        );
    }

    #[test]
    fn tool_call_inside_fenced_code_block_with_language_tag_is_not_executed() {
        let text = "Example:\n```rust\n[read_file: src/main.rs]\n```\nDone.";
        let calls = parse_all_tool_inputs(text);
        assert!(
            calls.is_empty(),
            "tool syntax inside fenced block must not execute: {calls:?}"
        );
    }

    #[test]
    fn block_tool_inside_code_fence_is_not_executed() {
        let text = "Use this form:\n```\n[write_file]\npath: foo.rs\n---content---\nhello\n[/write_file]\n```";
        let calls = parse_all_tool_inputs(text);
        assert!(
            calls.is_empty(),
            "block tool syntax inside code fence must not execute: {calls:?}"
        );
    }

    #[test]
    fn tool_call_outside_code_fence_still_executes() {
        // A real tool call that appears outside any code fence must still work.
        let text = "Let me check.\n[read_file: src/main.rs]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1, "real tool call outside fence must execute");
        assert!(matches!(&calls[0], ToolInput::ReadFile { path } if path == "src/main.rs"));
    }

    #[test]
    fn tool_call_after_code_fence_executes() {
        // Tool call appears AFTER a code fence block — not inside it.
        let text = "Some example:\n```\nfoo bar\n```\nNow for real:\n[list_dir: src/]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1, "tool call after fence must execute");
        assert!(matches!(&calls[0], ToolInput::ListDir { path } if path == "src/"));
    }

    // Single-line bracket calls

    #[test]
    fn parses_read_file_call() {
        let text = "[read_file: src/main.rs]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::ReadFile { path } if path == "src/main.rs"));
    }

    #[test]
    fn parses_list_dir_call() {
        let text = "[list_dir: src/]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::ListDir { path } if path == "src/"));
    }

    #[test]
    fn list_dir_defaults_path_when_empty() {
        let text = "[list_dir: ]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::ListDir { path } if path == "."));
    }

    #[test]
    fn parses_search_code_call() {
        let text = "[search_code: fn main]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(
            matches!(&calls[0], ToolInput::SearchCode { query, path: None }
            if query == "fn main")
        );
    }

    #[test]
    fn parses_shell_call() {
        let text = "[shell: cargo test my_filter]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::Shell { command }
            if command == "cargo test my_filter"));
    }

    #[test]
    fn shell_call_inside_code_fence_is_not_executed() {
        let text = "Example:\n```\n[shell: cargo check]\n```";
        let calls = parse_all_tool_inputs(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn parses_git_status_call() {
        let text = "[git_status]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::GitStatus));
    }

    #[test]
    fn parses_git_diff_call() {
        let text = "[git_diff]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::GitDiff));
    }

    #[test]
    fn parses_git_log_call() {
        let text = "[git_log]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], ToolInput::GitLog));
    }

    #[test]
    fn git_status_call_inside_code_fence_is_not_executed() {
        let text = "Example:\n```\n[git_status]\n```";
        let calls = parse_all_tool_inputs(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn git_diff_call_inside_code_fence_is_not_executed() {
        let text = "Example:\n```\n[git_diff]\n```";
        let calls = parse_all_tool_inputs(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn git_log_call_inside_code_fence_is_not_executed() {
        let text = "Example:\n```\n[git_log]\n```";
        let calls = parse_all_tool_inputs(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn parses_multiple_bracket_calls_in_response() {
        let text = "Let me check.\n[read_file: a.rs]\nAnd also:\n[list_dir: src/]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 2);
        assert!(matches!(&calls[0], ToolInput::ReadFile { path } if path == "a.rs"));
        assert!(matches!(&calls[1], ToolInput::ListDir { path } if path == "src/"));
    }

    // [search_code] block form (model-drift tolerance)

    #[test]
    fn parses_search_code_block_with_pattern_prefix() {
        let text = "[search_code]\npattern=logging\n[/search_code]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::SearchCode { query, path: None }
            if query == "logging")
        );
    }

    #[test]
    fn parses_search_code_block_with_pattern_colon_prefix() {
        // Model emits `pattern: log` (colon-space form) rather than `pattern=log`.
        let text = "[search_code]\npattern: log\n[/search_code]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::SearchCode { query, path: None }
            if query == "log")
        );
    }

    #[test]
    fn parses_search_code_block_with_query_colon_prefix() {
        let text = "[search_code]\nquery: fn main\n[/search_code]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::SearchCode { query, path: None }
            if query == "fn main")
        );
    }

    #[test]
    fn parses_search_code_block_with_query_prefix() {
        let text = "[search_code]\nquery=fn main\n[/search_code]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::SearchCode { query, path: None }
            if query == "fn main")
        );
    }

    #[test]
    fn parses_search_code_block_bare_text() {
        let text = "[search_code]\nfn main\n[/search_code]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::SearchCode { query, path: None }
            if query == "fn main")
        );
    }

    #[test]
    fn search_code_block_empty_body_is_skipped() {
        let text = "[search_code]\n   \n[/search_code]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn search_code_block_missing_close_tag_is_skipped() {
        let text = "[search_code]\npattern=logging";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn search_code_bracket_and_block_both_parse() {
        let text = "[search_code: logging]\n[search_code]\npattern=tracing\n[/search_code]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 2);
        assert!(matches!(&inputs[0], ToolInput::SearchCode { query, .. } if query == "logging"));
        assert!(matches!(&inputs[1], ToolInput::SearchCode { query, .. } if query == "tracing"));
    }

    #[test]
    fn read_file_missing_arg_is_skipped() {
        let text = "[read_file: ]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn bracket_call_newline_before_close_is_rejected() {
        let text = "[read_file: src/main.rs\n]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn path_may_contain_colon() {
        let text = "[read_file: /home/user/project/src/main.rs]";
        let calls = parse_all_tool_inputs(text);
        assert_eq!(calls.len(), 1);
        assert!(
            matches!(&calls[0], ToolInput::ReadFile { path } if path == "/home/user/project/src/main.rs")
        );
    }

    #[test]
    fn returns_empty_on_no_tool_calls() {
        assert!(parse_all_tool_inputs("Just a normal response.").is_empty());
    }

    // [write_file] blocks

    #[test]
    fn parses_valid_write_block() {
        let text =
            "[write_file]\npath: src/new.rs\n---content---\npub fn hello() {}\n[/write_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(matches!(&inputs[0], ToolInput::WriteFile { path, content }
            if path == "src/new.rs" && content == "pub fn hello() {}"));
    }

    #[test]
    fn write_block_missing_content_delimiter_is_skipped() {
        let text = "[write_file]\npath: src/new.rs\npub fn hello() {}\n[/write_file]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn write_block_missing_close_tag_is_skipped() {
        let text = "[write_file]\npath: src/new.rs\n---content---\ncontent";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn write_block_preserves_multiline_content() {
        let text = "[write_file]\npath: src/new.rs\n---content---\nuse std::fs;\n\npub fn hello() {\n    println!(\"hi\");\n}\n[/write_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        let ToolInput::WriteFile { content, .. } = &inputs[0] else {
            panic!("expected WriteFile");
        };
        assert!(content.contains("use std::fs;"));
        assert!(content.contains("println!(\"hi\")"));
        assert!(content.contains('\n'));
    }

    #[test]
    fn parses_write_file_bracket_form() {
        let text = "[write_file: src/new.rs]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(matches!(&inputs[0], ToolInput::WriteFile { path, content }
            if path == "src/new.rs" && content.is_empty()));
    }

    #[test]
    fn parses_write_file_bracket_form_with_path_prefix() {
        let text = "[write_file: path=src/new.rs]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(matches!(&inputs[0], ToolInput::WriteFile { path, content }
            if path == "src/new.rs" && content.is_empty()));
    }

    #[test]
    fn write_file_bracket_empty_arg_is_skipped() {
        let text = "[write_file: ]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn write_file_bracket_path_prefix_only_is_skipped() {
        let text = "[write_file: path=]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn write_file_bracket_and_block_coexist() {
        let text = "[write_file: empty.rs]\n[write_file]\npath: full.rs\n---content---\nhello\n[/write_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 2);
        assert!(matches!(&inputs[0], ToolInput::WriteFile { path, content }
            if path == "empty.rs" && content.is_empty()));
        assert!(matches!(&inputs[1], ToolInput::WriteFile { path, content }
            if path == "full.rs" && content == "hello"));
    }

    #[test]
    fn write_block_absolute_path_is_accepted() {
        // Regression: model was observed emitting absolute paths.
        let text =
            "[write_file]\npath: /Users/user/project/test.txt\n---content---\nhello\n[/write_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(matches!(&inputs[0], ToolInput::WriteFile { path, .. }
            if path == "/Users/user/project/test.txt"));
    }

    // [edit_file] blocks

    #[test]
    fn parses_valid_edit_block() {
        let text = "[edit_file]\npath: src/lib.rs\n---search---\nfn old() {}\n---replace---\nfn new() {}\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::EditFile { path, search, replace }
            if path == "src/lib.rs" && search == "fn old() {}" && replace == "fn new() {}")
        );
    }

    #[test]
    fn edit_block_missing_search_delimiter_produces_empty_search() {
        // When ---search--- is absent but ---replace--- is present, the block is parsed
        // with an empty search string. The tool's run() then returns a clear error
        // ("search text must not be empty") rather than silently discarding the block.
        let text = "[edit_file]\npath: src/lib.rs\n---replace---\nfn new() {}\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::EditFile { path, search, replace }
            if path == "src/lib.rs" && search.is_empty() && replace == "fn new() {}")
        );
    }

    #[test]
    fn edit_block_missing_replace_delimiter_is_skipped() {
        let text = "[edit_file]\npath: src/lib.rs\n---search---\nfn old() {}\n[/edit_file]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn edit_block_missing_close_tag_is_skipped() {
        let text = "[edit_file]\npath: src/lib.rs\n---search---\nold\n---replace---\nnew";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn edit_block_replace_delim_inside_search_content_is_handled_correctly() {
        // ---replace--- appearing mid-line inside the search text must not be treated as the delimiter.
        let text = "[edit_file]\npath: src/lib.rs\n---search---\n// see ---replace--- below\n---replace---\n// fixed\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        let ToolInput::EditFile {
            search, replace, ..
        } = &inputs[0]
        else {
            panic!("expected EditFile");
        };
        assert_eq!(search, "// see ---replace--- below");
        assert_eq!(replace, "// fixed");
    }

    #[test]
    fn edit_block_conflict_style_markers_are_accepted() {
        // Model emits <<<<<<< SEARCH / ======= / >>>>>>> REPLACE instead of ---search---/---replace---.
        // The parser must accept this and extract search/replace correctly.
        let text = "[edit_file]\npath: src/lib.rs\n<<<<<<< SEARCH\nfn old() {}\n=======\nfn new() {}\n>>>>>>> REPLACE\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(
            inputs.len(),
            1,
            "conflict-style edit block must parse: {inputs:?}"
        );
        assert!(
            matches!(&inputs[0], ToolInput::EditFile { path, search, replace }
            if path == "src/lib.rs" && search == "fn old() {}" && replace == "fn new() {}")
        );
    }

    #[test]
    fn edit_block_conflict_style_multiline() {
        let text = "[edit_file]\npath: src/lib.rs\n<<<<<<< SEARCH\nfn old() {\n    1\n}\n=======\nfn new() {\n    2\n}\n>>>>>>> REPLACE\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        let ToolInput::EditFile {
            search, replace, ..
        } = &inputs[0]
        else {
            panic!()
        };
        assert!(search.contains("fn old()") && search.contains("1"));
        assert!(replace.contains("fn new()") && replace.contains("2"));
    }

    #[test]
    fn edit_block_old_new_content_labels_are_accepted() {
        let text = "[edit_file]\npath: test_phase82.txt\nold content: hello world\nnew content: hello thunk\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::EditFile { path, search, replace }
            if path == "test_phase82.txt" && search == "hello world" && replace == "hello thunk")
        );
    }

    #[test]
    fn edit_block_old_new_content_labels_support_multiline_values() {
        let text = "[edit_file]\npath: src/lib.rs\nold content:\nfn old() {\n    println!(\"old\");\n}\nnew content:\nfn new() {\n    println!(\"new\");\n}\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], ToolInput::EditFile { path, search, replace }
            if path == "src/lib.rs" && search.contains("println!(\"old\")") && replace.contains("println!(\"new\")"))
        );
    }

    #[test]
    fn edit_block_generic_delimiters_accepted() {
        // Model derived delimiter names from prompt placeholder text instead of using
        // the canonical ---search---/---replace--- markers. Must still parse correctly.
        let text = "[edit_file]\npath: test_phase82.txt\n---text to find---\nhello world\n---replacement text---\nhello thunk\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(
            inputs.len(),
            1,
            "generic delimiter edit block must parse: {inputs:?}"
        );
        assert!(
            matches!(&inputs[0], ToolInput::EditFile { path, search, replace }
            if path == "test_phase82.txt" && search == "hello world" && replace == "hello thunk")
        );
    }

    #[test]
    fn edit_block_generic_delimiters_multiline_content() {
        let text = "[edit_file]\npath: src/lib.rs\n---find---\nfn old() {\n    1\n}\n---with---\nfn new() {\n    2\n}\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        let ToolInput::EditFile {
            search, replace, ..
        } = &inputs[0]
        else {
            panic!()
        };
        assert!(search.contains("fn old()") && search.contains("1"));
        assert!(replace.contains("fn new()") && replace.contains("2"));
    }

    #[test]
    fn edit_block_generic_delimiters_single_delimiter_is_skipped() {
        // Only one triple-dash delimiter — cannot determine search vs replace boundary.
        let text = "[edit_file]\npath: src/lib.rs\n---find---\nhello\n[/edit_file]";
        assert!(parse_all_tool_inputs(text).is_empty());
    }

    #[test]
    fn edit_block_preserves_multiline_content() {
        let text = "[edit_file]\npath: src/lib.rs\n---search---\nfn old() {\n    println!(\"old\");\n}\n---replace---\nfn new() {\n    println!(\"new\");\n}\n[/edit_file]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 1);
        let ToolInput::EditFile {
            search, replace, ..
        } = &inputs[0]
        else {
            panic!("expected EditFile");
        };
        assert!(search.contains("println!(\"old\")"));
        assert!(search.contains('\n'));
        assert!(replace.contains("println!(\"new\")"));
        assert!(replace.contains('\n'));
    }

    // Document order across mixed call types

    #[test]
    fn mixed_blocks_preserve_document_order() {
        let text = "\
[read_file: a.rs]\n\
[edit_file]\npath: b.rs\n---search---\nold\n---replace---\nnew\n[/edit_file]\n\
[write_file]\npath: c.rs\n---content---\nhello\n[/write_file]";

        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 3);
        assert!(matches!(&inputs[0], ToolInput::ReadFile { path } if path == "a.rs"));
        assert!(matches!(&inputs[1], ToolInput::EditFile { path, .. } if path == "b.rs"));
        assert!(matches!(&inputs[2], ToolInput::WriteFile { path, .. } if path == "c.rs"));
    }

    #[test]
    fn write_before_read_in_document_order() {
        let text = "[write_file]\npath: first.rs\n---content---\nhello\n[/write_file]\n[read_file: second.rs]";
        let inputs = parse_all_tool_inputs(text);
        assert_eq!(inputs.len(), 2);
        assert!(matches!(&inputs[0], ToolInput::WriteFile { path, .. } if path == "first.rs"));
        assert!(matches!(&inputs[1], ToolInput::ReadFile { path } if path == "second.rs"));
    }
}
