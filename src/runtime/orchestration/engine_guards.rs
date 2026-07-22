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

/// Canned, content-free refusal openers. Deliberately short and specific: each entry is a
/// multi-word phrase shaped like the *opening* of a boilerplate decline, not a fragment that
/// could plausibly appear mid-sentence in a substantive answer (e.g. a caveat like "although I
/// cannot assist with deploying this, the code shows..."). A false positive here blocks a
/// legitimate answer, which is strictly worse than missing some refusal phrasing — see
/// Phase 50 slice 50.5 risk framing.
const EVIDENCE_DISCONNECTED_REFUSAL_PHRASES: &[&str] = &[
    "i cannot assist with that",
    "i can't assist with that",
    "i cannot assist with this",
    "i can't assist with this",
    "i'm unable to assist with that",
    "i am unable to assist with that",
    "i cannot help with that",
    "i can't help with that",
    "i don't have the ability to",
    "i do not have the ability to",
    "as an ai language model",
];

/// Below this many trimmed characters, a response is treated as degenerate rather than a
/// genuine terse answer. Deliberately very low: "Done." (5 chars) is this codebase's own
/// standard terse confirmation after evidence/mutation and appears throughout the test
/// suite, so the threshold must sit strictly below it — this only catches truly empty or
/// near-empty output (e.g. "", "." ), not a short-but-real answer.
const EVIDENCE_DISCONNECTED_MIN_LEN: usize = 4;

/// True when `response` looks like a content-free non-answer: either a canned refusal opener
/// with no connection to whatever was retrieved, or output too short to be a real answer.
///
/// This is intentionally phrase-based rather than a content-overlap check against the tool
/// results: the model is a stateless text emitter with no separate grounding/confidence
/// signal, so there is nothing more structural to check against (see Phase 50.5
/// investigation §6). It is also intentionally silent on *why* a decline is happening — it
/// cannot distinguish a genuine in-scope refusal from an evidence-discarding one by pattern
/// alone. That distinction is handled by the correction message the caller injects on first
/// violation, which asks the model to name a reason; only a bare, reason-free refusal or
/// empty output should ever match this function twice in a row.
pub(crate) fn is_evidence_disconnected_response(response: &str) -> bool {
    let trimmed = response.trim();
    if trimmed.chars().count() < EVIDENCE_DISCONNECTED_MIN_LEN {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    EVIDENCE_DISCONNECTED_REFUSAL_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
}
