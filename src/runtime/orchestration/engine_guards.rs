use std::path::Path;

use super::super::investigation::investigation::InvestigationMode;

/// Returns true when a usage-lookup investigation should use broad (whole-project)
/// evidence policy rather than path-scoped. Broad if no requested read path was
/// given and the path scope (if any) doesn't look like a specific file.
pub(crate) fn usage_lookup_is_broad(
    mode: InvestigationMode,
    requested_read_path: Option<&str>,
    investigation_path_scope: Option<&str>,
) -> bool {
    if !matches!(mode, InvestigationMode::UsageLookup) || requested_read_path.is_some() {
        return false;
    }

    match investigation_path_scope {
        None => true,
        Some(scope) => !path_scope_looks_like_file(scope),
    }
}

pub(crate) fn path_scope_looks_like_file(scope: &str) -> bool {
    Path::new(scope)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains('.'))
}

/// Extracts relative file-path tokens cited in a model answer.
/// Returns only tokens that look like project source paths: relative,
/// slash-separated, with a recognized file extension, no URL scheme, no `..`.
/// Used by the read-set answer guard to detect unread paths cited as evidence.
pub(crate) fn extract_claimed_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for raw in text.split(|c: char| {
        c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'')
    }) {
        // Strip surrounding punctuation that is never part of a file path.
        let token =
            raw.trim_matches(|c: char| matches!(c, '`' | ':' | '!' | '?' | '*' | '_' | ',' | ';'));
        let token = token.trim_end_matches('.');
        if token.is_empty() {
            continue;
        }
        // Must start with alphanumeric (excludes CLI flags like --path/to/x).
        if !token.chars().next().is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        // Must contain a path separator and must be relative.
        if !token.contains('/') || token.starts_with('/') {
            continue;
        }
        // Exclude URLs.
        if token.contains("://") {
            continue;
        }
        // Exclude parent-directory traversal.
        if token.split('/').any(|seg| seg == "..") {
            continue;
        }
        // Must have a file extension on the last segment: .ext where ext is 1–5 alpha chars.
        let last_seg = token.split('/').next_back().unwrap_or("");
        let has_ext = last_seg.rfind('.').is_some_and(|i| {
            let ext = &last_seg[i + 1..];
            !ext.is_empty() && ext.len() <= 5 && ext.bytes().all(|b| b.is_ascii_alphabetic())
        });
        if has_ext {
            paths.push(token.to_string());
        }
    }
    paths
}

pub(crate) fn is_definition_only_usage_answer(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains(" is defined in ")
        || lower.contains(" are defined in ")
        || lower.contains(" is declared in ")
        || lower.contains(" are declared in ")
}
