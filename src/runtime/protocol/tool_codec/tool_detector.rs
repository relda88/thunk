// Protocol guard

use super::tool_parser::{
    code_fence_ranges, find_bracket_close, is_line_isolated, NATIVE_SINGLE_LINE_TOOL_NAMES,
    NATIVE_STATIC_TOOL_NAMES,
};

/// Returns true for assistant messages that are tool-call requests rather than
/// natural-language responses. Tool calls begin with `[`, the opening bracket
/// of any single-line tool invocation in the wire format.
pub(crate) fn is_tool_call_message(content: &str) -> bool {
    content.trim_start().starts_with('[')
}

/// Returns true if the text contains a fabricated tool result or error block.
/// Assistant output must never contain these — they are runtime-injected only.
/// Used by the engine to detect and surface model misbehavior rather than
/// silently accepting a fabricated result as a valid direct answer.
pub fn contains_fabricated_exchange(text: &str) -> bool {
    text.contains("=== tool_result:") || text.contains("=== tool_error:")
}

/// Returns true when an assistant response contains edit_file tag syntax (both open and close
/// tags are present) but the block could not be parsed into a valid ToolInput. This fingerprints
/// garbled edit repair attempts where the model included `[edit_file]...[/edit_file]` but used
/// unrecognized delimiter names or no delimiters at all. Used by the engine to inject a targeted
/// correction rather than silently accepting the response as a Direct answer.
pub fn contains_edit_attempt(text: &str) -> bool {
    text.contains("[edit_file]") && text.contains("[/edit_file]")
}

/// Returns true if the text contains an unmatched block tool tag — either a known CLOSE tag
/// without a matching open, or a known OPEN tag without a matching close.
///
/// Two drift patterns are detected:
/// - Close-without-open: model used a wrong opening tag name (e.g. `[test_file]...[/write_file]`).
/// - Open-without-close: model emitted the opening tag inline without a body/close
///   (e.g. `[write_file] path: foo ---content--- bar` with no `[/write_file]`).
///
/// Both patterns produce zero parsed tool calls and must be corrected rather than silently
/// accepted as a direct text answer.
/// Returns the name of the mutation tool detected in an open-without-close pattern,
/// used to specialize the correction message with the tool's exact required syntax.
/// Returns None when the pattern is close-without-open (wrong tag name drift) or
/// when neither edit_file nor write_file is involved.
pub fn detected_malformed_mutation_tool(text: &str) -> Option<&'static str> {
    if text.contains("[edit_file]") && !text.contains("[/edit_file]") {
        Some("edit_file")
    } else if text.contains("[write_file]") && !text.contains("[/write_file]") {
        Some("write_file")
    } else {
        None
    }
}

pub fn contains_malformed_block(text: &str) -> bool {
    (text.contains("[/write_file]") && !text.contains("[write_file]"))
        || (text.contains("[/edit_file]") && !text.contains("[edit_file]"))
        || (text.contains("[/search_code]") && !text.contains("[search_code]"))
        || (text.contains("[write_file]") && !text.contains("[/write_file]"))
        || (text.contains("[edit_file]") && !text.contains("[/edit_file]"))
}

/// Bare block-tag literals that are legitimately the opening tag of a multi-line block
/// call (write_file/search_code both also have a single-line "[name: args]" form). These
/// exact literals are already owned by `contains_malformed_block`/`contains_edit_attempt`
/// and must never be treated as a failed single-line attempt by this detector.
const EXCLUDED_BARE_BLOCK_OPEN_TAGS: &[&str] = &["[write_file]", "[search_code]"];

/// Detects a single-line bracket call whose tool name is recognized (a native tool, or a
/// dynamically-registered MCP tool name) but whose syntax does not match the grammar the
/// real scanners accept — missing colon, wrong case, a space before the colon, or a
/// genuinely unrecoverable missing closing bracket. Mirrors `contains_malformed_block` /
/// `detected_malformed_mutation_tool`'s shape for the block-form tools, but for the
/// single-line "[name: args]" grammar instead. Returns the canonical (correctly-cased)
/// tool name for the correction message, or None if no such attempt is present.
///
/// Two conditions keep this from over-firing on unrelated or illustrative text:
/// - Fenced code is excluded, same as the real scanners — illustrative examples inside
///   ``` blocks remain legal and are never flagged.
/// - A candidate must be alone on its line (see `is_line_isolated`). Prose that merely
///   mentions tool syntax mid-sentence (e.g. "Use the syntax [read_file: <path>] to read
///   a file") is not an attempt to invoke the tool and must not be flagged.
///
/// Deliberately does not flag an empty argument after an otherwise well-formed call (e.g.
/// "[read_file: ]") — that is pre-existing, already-tested accepted-skip behavior handled
/// by the scanners themselves, not a syntax failure this detector is scoped to catch.
pub fn detected_malformed_bracket_call(text: &str, dynamic_names: &[&str]) -> Option<String> {
    let fences = code_fence_ranges(text);
    let mut search_start = 0;
    while let Some(rel) = text[search_start..].find('[') {
        let open = search_start + rel;
        if fences.iter().any(|&(s, e)| open >= s && open < e) {
            search_start = open + 1;
            continue;
        }

        let after_open = &text[open + 1..];
        let colon_tool_names = NATIVE_SINGLE_LINE_TOOL_NAMES
            .iter()
            .chain(dynamic_names.iter());

        // Colon-form tools: "[name:" is the real grammar's exact prefix.
        let colon_match = colon_tool_names.map(|n| (*n, true)).find(|(n, _)| {
            after_open.len() >= n.len() && after_open[..n.len()].eq_ignore_ascii_case(n)
        });
        // Static no-colon tools: "[name]" is the real grammar's exact literal.
        let static_match = NATIVE_STATIC_TOOL_NAMES
            .iter()
            .map(|n| (*n, false))
            .find(|(n, _)| {
                after_open.len() >= n.len() && after_open[..n.len()].eq_ignore_ascii_case(n)
            });

        let Some((known_name, is_colon_form)) = colon_match.or(static_match) else {
            search_start = open + 1;
            continue;
        };

        // Boundary check: the matched name must end at ':', ']', whitespace, or end of
        // text — otherwise this is a longer, unrelated identifier that merely starts with
        // a known tool name (e.g. a hypothetical "search_code_extra").
        let boundary_ok = after_open[known_name.len()..]
            .chars()
            .next()
            .is_none_or(|c| c == ':' || c == ']' || c.is_whitespace());
        if !boundary_ok {
            search_start = open + 1;
            continue;
        }

        if EXCLUDED_BARE_BLOCK_OPEN_TAGS
            .iter()
            .any(|tag| text[open..].starts_with(tag))
        {
            search_start = open + 1;
            continue;
        }

        let exact_prefix_matches = if is_colon_form {
            text[open..].starts_with(&format!("[{known_name}:"))
        } else {
            text[open..].starts_with(&format!("[{known_name}]"))
        };

        if exact_prefix_matches {
            if !is_colon_form {
                // Exact static literal — always a valid, already-handled match.
                search_start = open + known_name.len() + 2;
                continue;
            }
            // Exact colon prefix — already recognized by the real grammar. Only flag if
            // the argument's closing bracket is genuinely unrecoverable (Fix 1/2
            // territory); any well-formed or intentionally-empty-arg outcome is already
            // handled elsewhere and out of scope here.
            let after_colon = open + known_name.len() + 2;
            let has_valid_close = matches!(
                find_bracket_close(text, after_colon),
                Some(close) if !text[after_colon..close].contains('\n')
            );
            if has_valid_close {
                search_start = after_colon;
                continue;
            }
        }

        // Determine a best-effort span for the line-isolation check: up to the next ']'
        // on the same line if one exists, otherwise to the end of the line.
        let line_end = text[open..]
            .find('\n')
            .map(|i| open + i)
            .unwrap_or(text.len());
        let span_end = text[open..line_end]
            .find(']')
            .map(|i| open + i + 1)
            .unwrap_or(line_end);

        if !is_line_isolated(text, open, span_end) {
            search_start = open + 1;
            continue;
        }

        return Some(known_name.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_fabricated_exchange_detects_tool_result_blocks() {
        assert!(contains_fabricated_exchange(
            "=== tool_result: read_file ===\nsome content\n=== /tool_result ==="
        ));
        assert!(contains_fabricated_exchange(
            "=== tool_error: read_file ===\nfailed\n=== /tool_error ==="
        ));
        assert!(!contains_fabricated_exchange("[read_file: src/main.rs]"));
        assert!(!contains_fabricated_exchange("Here is my answer."));
    }

    // contains_malformed_block

    #[test]
    fn malformed_block_detected_when_close_tag_has_no_matching_open() {
        // The drift case: model used wrong opening tag, correct closing tag
        assert!(contains_malformed_block(
            "[test_file]\npath: f.txt\n---content---\nhello\n[/write_file]"
        ));
        assert!(contains_malformed_block(
            "[wrong]\npath: f.rs\n---search---\nx\n---replace---\ny\n[/edit_file]"
        ));
        assert!(contains_malformed_block(
            "[unknown]\npattern: log\n[/search_code]"
        ));
    }

    #[test]
    fn malformed_block_not_triggered_by_correct_blocks() {
        // Correctly formed blocks have both open and close tags — not malformed
        assert!(!contains_malformed_block(
            "[write_file]\npath: f.txt\n---content---\nhello\n[/write_file]"
        ));
        assert!(!contains_malformed_block(
            "[edit_file]\npath: f.rs\n---search---\nx\n---replace---\ny\n[/edit_file]"
        ));
        assert!(!contains_malformed_block(
            "[search_code]\npattern=log\n[/search_code]"
        ));
    }

    #[test]
    fn malformed_block_not_triggered_by_plain_responses() {
        assert!(!contains_malformed_block("Here is my answer."));
        assert!(!contains_malformed_block("[read_file: src/main.rs]"));
    }

    // detected_malformed_bracket_call (Fix 3, Slice 50.6)

    #[test]
    fn missing_colon_is_detected() {
        assert_eq!(
            detected_malformed_bracket_call("[read_file]", &[]),
            Some("read_file".to_string())
        );
    }

    #[test]
    fn case_variation_is_detected() {
        assert_eq!(
            detected_malformed_bracket_call("[Read_file: src/main.rs]", &[]),
            Some("read_file".to_string())
        );
    }

    #[test]
    fn space_before_colon_is_detected() {
        assert_eq!(
            detected_malformed_bracket_call("[read_file : src/main.rs]", &[]),
            Some("read_file".to_string())
        );
    }

    #[test]
    fn well_formed_call_is_not_flagged() {
        assert_eq!(
            detected_malformed_bracket_call("[read_file: src/main.rs]", &[]),
            None
        );
    }

    #[test]
    fn unrelated_bracket_text_is_not_flagged() {
        // Not a registered tool name — genuinely unrelated bracket text, not a failed
        // attempt at a known tool.
        assert_eq!(
            detected_malformed_bracket_call("[unknown_tool: some argument]", &[]),
            None
        );
    }

    #[test]
    fn fenced_malformed_text_is_not_flagged() {
        let text = "Here is an example:\n```\n[read_file]\n```\nThat is illustrative only.";
        assert_eq!(detected_malformed_bracket_call(text, &[]), None);
    }

    #[test]
    fn bare_block_open_tags_are_excluded_write_file_and_search_code() {
        // The bare literal is legitimately the block-form open tag, already owned by
        // contains_malformed_block/contains_edit_attempt — must not double-flag here.
        assert_eq!(
            detected_malformed_bracket_call(
                "[write_file]\npath: f.txt\n---content---\nhello\n[/write_file]",
                &[]
            ),
            None
        );
        assert_eq!(
            detected_malformed_bracket_call("[search_code]\npattern: log\n[/search_code]", &[]),
            None
        );
    }

    #[test]
    fn malformed_write_file_variant_with_argument_is_still_flagged() {
        // Unlike the bare "[write_file]" open tag, this clearly attempts the single-line
        // form with an argument and missing colon — still a genuine malformed attempt.
        assert_eq!(
            detected_malformed_bracket_call("[write_file src/new.rs]", &[]),
            Some("write_file".to_string())
        );
    }

    #[test]
    fn illustrative_prose_describing_syntax_is_not_flagged() {
        // Fix 5 interaction: a call embedded mid-sentence is prose, not an attempt, and
        // must not be flagged even when the bracket text is malformed.
        let text = "Try [read_file] to load it now.";
        assert_eq!(detected_malformed_bracket_call(text, &[]), None);
    }

    #[test]
    fn genuinely_unclosed_bracket_is_flagged() {
        // Fix 1/2 territory: the real scanner cannot find a valid closing bracket at all,
        // so this must surface via this detector rather than silently falling through.
        let text = "[read_file: unterminated with no closing bracket anywhere";
        assert_eq!(
            detected_malformed_bracket_call(text, &[]),
            Some("read_file".to_string())
        );
    }

    #[test]
    fn dynamic_mcp_name_missing_colon_is_detected() {
        // The original incident repro: an MCP tool name emitted with no colon at all.
        let text = "[mcp::filesystem::list_allowed_directories]";
        let dynamic_names = ["mcp::filesystem::list_allowed_directories"];
        assert_eq!(
            detected_malformed_bracket_call(text, &dynamic_names),
            Some("mcp::filesystem::list_allowed_directories".to_string())
        );
    }

    #[test]
    fn unregistered_dynamic_looking_name_is_not_flagged() {
        // Same shape as the incident repro, but the name is not in dynamic_names for this
        // turn — must not be treated as a known-tool-name attempt.
        let text = "[mcp::filesystem::list_allowed_directories]";
        assert_eq!(detected_malformed_bracket_call(text, &[]), None);
    }
}
