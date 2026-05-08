// Protocol guard

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
}
