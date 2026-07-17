use super::super::paths::normalize_evidence_path;

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "cpp", "h", "hpp", "yaml", "yml",
    "toml", "json", "ini", "cfg", "conf", "md",
];

/// Determines whether a prompt should enter investigation mode.
///
/// Uses structural signals first (identifier-like tokens), then falls back to
/// constrained natural-language lookup detection. This must remain conservative
/// to avoid over-triggering investigation on general questions.
pub(crate) fn prompt_requires_investigation(text: &str) -> bool {
    for raw in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '.'
                    | '?'
                    | '!'
                    | ';'
                    | ':'
                    | '"'
                    | '\''
                    | '`'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
            )
    }) {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        if is_snake_case_identifier(token) || is_pascal_case_identifier(token) {
            return true;
        }
    }

    if prompt_contains_code_file_token(text) {
        return true;
    }

    natural_language_code_lookup_requires_investigation(text)
}

/// Returns true when the prompt contains a whitespace-delimited token whose file
/// extension is in the recognized source or config extension set.
///
/// Uses whitespace splitting (not the identifier splitter) so "engine.rs" is not
/// fragmented. Strips trailing punctuation before extension matching to handle
/// "engine.rs?" and "engine.rs,".
///
/// Intentionally narrow: only fires on recognized extensions so that version
/// strings like "3.14" or "v2.3" do not match.
fn prompt_contains_code_file_token(text: &str) -> bool {
    for token in text.split_whitespace() {
        let stripped = token.trim_end_matches(|c: char| {
            matches!(
                c,
                '.' | ',' | '?' | '!' | ';' | ':' | ')' | ']' | '}' | '"' | '\''
            )
        });
        if let Some(ext) = std::path::Path::new(stripped)
            .extension()
            .and_then(|e| e.to_str())
        {
            if CODE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                return true;
            }
        }
    }
    false
}

/// Detects investigation intent from natural-language lookup phrasing.
///
/// Requires both a lookup verb (find/where/locate/search) and a secondary
/// condition indicating code-related intent, except for "search" which is
/// treated as an explicit tool request.
fn natural_language_code_lookup_requires_investigation(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let has_lookup_verb = contains_word(&lower, "find")
        || contains_word(&lower, "where")
        || contains_word(&lower, "locate")
        || contains_word(&lower, "search")
        || contains_word(&lower, "trace")
        || contains_word(&lower, "follow");
    if !has_lookup_verb {
        return false;
    }

    // "search" is a self-sufficient trigger — it is an explicit request to run the search tool.
    // "find/where/locate" still require a secondary condition to avoid false positives on
    // conversational phrasing like "find a good approach".
    if contains_word(&lower, "search") {
        return true;
    }

    // "where is/are the <code-noun>" is a self-sufficient trigger — these nouns are
    // unambiguously code-domain references that don't need an additional secondary verb.
    if contains_word(&lower, "where") {
        const CODE_NOUNS: &[&str] = &[
            "function", "method", "module", "class", "struct", "enum", "trait", "type", "variable",
            "constant", "file", "command", "tool",
        ];
        if CODE_NOUNS.iter().any(|noun| contains_word(&lower, noun)) {
            return true;
        }
    }

    [
        "defined",
        "implemented",
        "initialize",
        "initialized",
        "initialization",
        "initialised",
        "configured",
        "create",
        "created",
        "creation",
        "register",
        "registered",
        "registration",
        "load",
        "loaded",
        "loading",
        "save",
        "saved",
        "saving",
        "stored",
        "handled",
        "called",
        "used",
        // occurrence/appearance phrasing: "find all occurrences of X", "where it appears"
        "occur",
        "occurs",
        "occurrence",
        "occurrences",
        "appear",
        "appears",
        "filtered",
        "rendered",
        // architectural traversal / relational verbs: "find how X reaches Y", "find how X interacts with Y"
        "reaches",
        "reach",
        "interacts",
        "interact",
        "connects",
        "connect",
    ]
    .iter()
    .any(|term| contains_word(&lower, term))
}

/// Checks for exact token matches within a normalized text stream.
///
/// Avoids substring matching to prevent false positives (e.g., "find" in "finder").
fn contains_word(text: &str, needle: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|token| token == needle)
}

/// Produces a normalized token stream for prompt analysis.
///
/// Lowercases and splits on non-identifier characters. Shared by multiple
/// classification helpers to ensure consistent tokenization.
pub(crate) fn normalized_prompt_tokens(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

/// Detects whether the user is requesting a mutation operation.
///
/// Uses a strict keyword list to avoid accidental triggering from
/// conversational language.
pub(crate) fn user_requested_mutation(text: &str) -> bool {
    text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '.'
                    | '?'
                    | '!'
                    | ';'
                    | ':'
                    | '"'
                    | '\''
                    | '`'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '/'
                    | '\\'
            )
    })
    .any(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "add"
                | "change"
                | "create"
                | "delete"
                | "edit"
                | "modify"
                | "overwrite"
                | "replace"
                | "update"
                | "write"
        )
    })
}

/// Returns the fact text if the user message is a "remember this" intent.
/// Triggers on: "remember ...", "don't forget ...", "note that ..."
pub(crate) fn user_requested_remember(text: &str) -> Option<String> {
    let lower = text.trim().to_lowercase();
    let prefixes = [
        "remember that ",
        "remember ",
        "remember: ",
        "dont forget that ",
        "dont forget ",
        "don't forget ",
        "note that ",
    ];
    for prefix in &prefixes {
        if lower.starts_with(prefix) {
            let extracted = text.trim()[prefix.len()..].trim();
            return Some(
                extracted
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string(),
            );
        }
    }
    None
}

pub(crate) fn user_requested_execution(text: &str) -> bool {
    text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '.'
                    | '?'
                    | '!'
                    | ';'
                    | ':'
                    | '"'
                    | '\''
                    | '`'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '/'
                    | '\\'
            )
    })
    .any(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "run" | "execute" | "cargo" | "check" | "build" | "test" | "clippy"
        )
    })
}

pub(crate) fn requested_shell_command(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let prefixes = ["run ", "execute "];
    for prefix in prefixes {
        if let Some(rest) = lower.find(prefix).map(|i| &text[i + prefix.len()..]) {
            let cmd = strip_command_preamble(rest.trim());
            if !cmd.is_empty() {
                return Some(cmd.to_string());
            }
        }
    }
    None
}

/// Strips conversational preambles from an extracted command so that
/// "run the shell command: grep foo" yields "grep foo", not "the shell command: grep foo".
/// Longest, most-specific preambles are checked first.
fn strip_command_preamble(cmd: &str) -> &str {
    let lower = cmd.to_ascii_lowercase();
    let preambles = [
        "the shell command: ",
        "shell command: ",
        "the command: ",
        "command: ",
    ];
    for preamble in preambles {
        if lower.starts_with(preamble) {
            return cmd[preamble.len()..].trim_start();
        }
    }
    cmd
}

// Retained as the cargo-only allowlist for the NL seeding path's legacy semantics and
// for documentation of the permitted-command policy; the active shell-seed gate is now
// the tier classifier. Kept intact per Slice 47.5 (no behavioral callers at present).
#[allow(dead_code)]
pub(crate) fn is_permitted_shell_command(cmd: &str) -> bool {
    let first_token = cmd.split_whitespace().next().unwrap_or("");
    matches!(first_token, "cargo")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SimpleEditRequest {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Extracts a narrow natural-language edit request for weak-model stabilization.
///
/// Accepted forms only:
/// - "Edit the file <path> replace the content <old> with <new>"
/// - "Edit <path> replace <old> with <new>"
/// - "Edit <path> and change <old> to <new>"
/// - "Edit <path> to change <old> to <new>"
/// - "In <path> change <old> to <new>"
pub(crate) fn requested_simple_edit(text: &str) -> Option<SimpleEditRequest> {
    // (prefix, change_marker, end_marker)
    const PATTERNS: &[(&str, &str, &str)] = &[
        ("edit the file ", " replace the content ", " with "),
        ("edit ", " replace ", " with "),
        ("edit ", " and change ", " to "),
        ("edit ", " change ", " to "),
        ("edit the file ", " change ", " to "),
        ("edit ", " to change ", " to "),
        ("in ", " change ", " to "),
    ];

    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();

    for &(prefix, change_marker, end_marker) in PATTERNS {
        if !lower.starts_with(prefix) {
            continue;
        }
        let rest = &trimmed[prefix.len()..];
        let lower_rest = &lower[prefix.len()..];

        let change_index = match lower_rest.find(change_marker) {
            Some(i) => i,
            None => continue,
        };

        let path = rest[..change_index].trim_matches(|c: char| {
            matches!(
                c,
                '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        });
        if path.is_empty() || path.chars().any(char::is_whitespace) || !looks_like_file_path(path) {
            continue;
        }

        let remainder = &rest[change_index + change_marker.len()..];
        let lower_remainder = &lower_rest[change_index + change_marker.len()..];

        let end_index = match lower_remainder.find(end_marker) {
            Some(i) => i,
            None => continue,
        };

        let search = remainder[..end_index].trim();
        let replace = remainder[end_index + end_marker.len()..].trim();
        if search.is_empty() || replace.is_empty() {
            continue;
        }

        return Some(SimpleEditRequest {
            path: path.to_string(),
            search: search.to_string(),
            replace: replace.to_string(),
        });
    }

    None
}

/// Extracts a single relative path scope from an investigation prompt.
///
/// Fires only on the conservative pattern `in <token>` / `within <token>`, with
/// an optional `the` before the token, where the token contains `/`, has no
/// whitespace, and is not a URL. Trailing punctuation
/// that is not part of a path is stripped. Returns `None` when the pattern is absent
/// or ambiguous (multiple qualifying tokens, empty token after stripping, etc.).
///
/// Examples that match:
///   "Where is TaskStatus handled in sandbox/cli/"     → Some("sandbox/cli/")
///   "Find logging in sandbox/services/"               → Some("sandbox/services/")
///   "Find where database is configured in the sandbox/ folder" → Some("sandbox/")
///
/// Examples that do not match:
///   "Find X in the application"  → None  (no `/` in token)
///   "Find X in context"          → None  (no `/`)
///   "Find X in https://…"        → None  (URL rejected)
pub(crate) fn extract_investigation_path_scope(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();
    let lower_words: Vec<&str> = lower.split_whitespace().collect();

    let mut found: Option<String> = None;

    for (i, lw) in lower_words.iter().enumerate() {
        if (*lw == "in" || *lw == "within") && i + 1 < words.len() {
            let next = i + 1;
            let path_index = if lower_words[next] == "the" && next + 1 < words.len() {
                next + 1
            } else {
                next
            };
            let raw = words[path_index];
            // Strip trailing punctuation that cannot be part of a relative path.
            let stripped = raw.trim_end_matches(|c: char| {
                matches!(
                    c,
                    '.' | ',' | '?' | '!' | ';' | ':' | ')' | ']' | '}' | '"' | '\''
                )
            });
            if stripped.is_empty() {
                continue;
            }
            // Require at least one `/` — distinguishes paths from plain words.
            if !stripped.contains('/') {
                continue;
            }
            // Reject URLs.
            if stripped.starts_with("http://") || stripped.starts_with("https://") {
                continue;
            }
            // Reject anything with embedded whitespace (shouldn't happen after split, but be safe).
            if stripped.contains(|c: char| c.is_whitespace()) {
                continue;
            }
            // More than one qualifying token → ambiguous; return None.
            if found.is_some() {
                return None;
            }
            found = Some(normalize_evidence_path(stripped));
        }
    }

    found
}

/// Extracts a bare filename (no slash) with a recognized code extension from an
/// explanation-verb prompt, to be used as a direct-read target.
///
/// Fires only on "what does", "explain", or "describe" prefixes — not on lookup
/// verbs like "find" or "where", which follow a different investigation path.
/// Returns None when zero or more than one qualifying token is found.
///
/// Examples that match:
///   "What does task_service.py do?"  → Some("task_service.py")
///   "Explain engine.rs"              → Some("engine.rs")
///   "Describe config.toml"           → Some("config.toml")
///
/// Examples that do not match:
///   "What does sandbox/services/task_service.py do?"  → None  (has slash, handled by path_from_explicit_file_prompt)
///   "What does task_service.py and user_service.py do?" → None (ambiguous)
///   "Find task_service.py in the codebase"             → None  (wrong verb)
fn path_from_bare_filename_explain_prompt(text: &str) -> Option<String> {
    let lower = text.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("what does ")
        || lower.starts_with("explain ")
        || lower.starts_with("describe ")
        || lower.starts_with("find what "))
    {
        return None;
    }

    let mut found: Option<String> = None;
    for token in text.split_whitespace() {
        let stripped = token
            .trim_matches(|c: char| {
                matches!(
                    c,
                    '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
                )
            })
            .trim_end_matches(['.', '?', '!']);

        if stripped.is_empty() || stripped.contains('/') || stripped.contains('\\') {
            continue;
        }
        let ext = match std::path::Path::new(stripped)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some(e) => e.to_ascii_lowercase(),
            None => continue,
        };
        if !CODE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(stripped.to_string());
    }

    found
}

/// Extracts a direct-read file path from a prompt starting with "read".
///
/// Accepts:
/// - "read <path>"
/// - "read file <path>"
/// - question/explanation-style prompts with exactly one explicit relative file path
///   such as "What does sandbox/services/task_service.py do?" or
///   "Explain sandbox/services/task_service.py"
///
/// Returns None if the structure does not match or the candidate does not
/// resemble a relative file path.
pub(crate) fn requested_read_path(text: &str) -> Option<String> {
    path_from_read_verb(text)
        .or_else(|| path_from_what_is_in_query(text))
        .or_else(|| path_from_explicit_file_prompt(text))
        .or_else(|| path_from_bare_filename_explain_prompt(text))
}

fn path_from_read_verb(text: &str) -> Option<String> {
    let mut tokens = text.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("read") {
        return None;
    }

    let mut candidate = tokens.next()?;
    if candidate.eq_ignore_ascii_case("file") {
        candidate = tokens.next()?;
    }

    let path = candidate.trim_matches(|c: char| {
        matches!(
            c,
            '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    });
    if looks_like_file_path(path) {
        Some(path.to_string())
    } else {
        None
    }
}

fn path_from_explicit_file_prompt(text: &str) -> Option<String> {
    let lower = text.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("what does ")
        || lower.starts_with("explain ")
        || lower.starts_with("find what "))
    {
        return None;
    }

    single_explicit_relative_file_path(text)
}

fn single_explicit_relative_file_path(text: &str) -> Option<String> {
    let mut found: Option<String> = None;

    for raw in text.split_whitespace() {
        let path = raw
            .trim_matches(|c: char| {
                matches!(
                    c,
                    '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
                )
            })
            .trim_end_matches(['.', '?', '!']);

        if !looks_like_explicit_relative_file_path(path) {
            continue;
        }

        let normalized = normalize_evidence_path(path);
        if found.is_some() {
            return None;
        }
        found = Some(normalized);
    }

    found
}

fn looks_like_explicit_relative_file_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.contains(|c: char| c.is_whitespace())
        || path.ends_with('/')
        || !path.contains('/')
        || !looks_like_file_path(path)
    {
        return false;
    }

    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains('.') || name.eq_ignore_ascii_case("README"))
}

/// Extracts a path-qualified direct-read target from "what is in <path>" queries.
///
/// Only fires when the path token contains `/` — bare filenames like "engine.rs"
/// are intentionally excluded because they are ambiguous and should enter
/// investigation mode instead.
///
/// Accepted forms:
///   "What is in src/runtime/engine.rs?"       → Some("src/runtime/engine.rs")
///   "What is in the sandbox/main.py?"         → Some("sandbox/main.py")
fn path_from_what_is_in_query(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if !lower.starts_with("what is in ") {
        return None;
    }
    let rest = &text["what is in ".len()..].trim_start();
    let mut tokens = rest.split_whitespace();
    let mut candidate = tokens.next()?;
    if matches!(candidate.to_ascii_lowercase().as_str(), "the" | "a" | "an") {
        candidate = tokens.next()?;
    }
    let path = candidate.trim_matches(|c: char| {
        matches!(
            c,
            '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '?' | '!'
        )
    });
    if path.contains('/') && looks_like_file_path(path) {
        Some(path.to_string())
    } else {
        None
    }
}

/// Heuristic check for whether a token resembles a file path.
///
/// Accepts separator-containing tokens ("src/tools/mod.rs", "src\\tui"),
/// extension-suffixed tokens ("Cargo.toml"), dotfiles (".gitignore"), and the
/// README special case — without resolving or validating against the filesystem.
/// Anything containing whitespace or sentence punctuation (`?`, `!`) is rejected,
/// so natural-language phrases that mention `/` or `.` are not misclassified.
///
/// Residual risk (accepted): whitespace-free NL fragments containing `/`
/// (e.g. "either/or") still pass. Whitespace rejection plus extension/segment
/// shape is sufficient here; full NL detection is out of scope.
pub(crate) fn looks_like_file_path(path: &str) -> bool {
    if path.is_empty()
        || path.chars().any(char::is_whitespace)
        || path.contains('?')
        || path.contains('!')
    {
        return false;
    }
    if path.eq_ignore_ascii_case("README") {
        return true;
    }
    if path.contains('/') || path.contains('\\') {
        return true;
    }
    has_extension_like_suffix(path)
}

/// True for `<stem>.<ext>` where the stem is word-like and the extension is
/// 1–6 alphanumerics ("Cargo.toml", "v1.2"), and for word-like dotfiles
/// (".gitignore", ".env.local").
fn has_extension_like_suffix(path: &str) -> bool {
    fn is_path_word_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
    }
    if let Some(rest) = path.strip_prefix('.') {
        return !rest.is_empty() && rest.chars().all(is_path_word_char);
    }
    let Some((stem, ext)) = path.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem.chars().all(is_path_word_char)
        && (1..=6).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Runtime-owned first-tool decision for RetrievalFirst turns.
///
/// Computed once from the original user prompt before the generation loop starts.
/// When non-None, the engine seeds `pending_runtime_call` directly — the model
/// never generates before the first tool executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectReadMode {
    Raw,
    Explain,
}

pub(crate) enum RetrievalIntent {
    None,
    DirectRead { path: String, mode: DirectReadMode },
    DirectoryListing { path: String },
}

/// Classifies a prompt into a runtime-owned retrieval intent.
///
/// Checks direct-read first (path-qualified "what is in" or "read" forms),
/// then directory navigation (nav verb + path token or structural cue).
/// Returns None when neither applies, including all investigation-required turns.
pub(crate) fn classify_retrieval_intent(text: &str) -> RetrievalIntent {
    if let Some((path, mode)) = classify_direct_read(text) {
        return RetrievalIntent::DirectRead { path, mode };
    }
    if let Some(path) = extract_directory_target(text) {
        return RetrievalIntent::DirectoryListing { path };
    }
    RetrievalIntent::None
}

fn classify_direct_read(text: &str) -> Option<(String, DirectReadMode)> {
    let mode = classify_direct_read_mode(text)?;
    if let Some(path) = requested_read_path(text) {
        return Some((path, mode));
    }
    if matches!(mode, DirectReadMode::Raw) {
        if let Some(path) = path_from_show_verb(text) {
            return Some((path, mode));
        }
    }
    None
}

fn classify_direct_read_mode(text: &str) -> Option<DirectReadMode> {
    let lower = text.trim_start().to_ascii_lowercase();
    if lower.starts_with("read ") || lower.starts_with("show ") || lower.starts_with("what is in ")
    {
        return Some(DirectReadMode::Raw);
    }
    if lower.starts_with("explain ")
        || lower.starts_with("what does ")
        || lower.starts_with("find what ")
    {
        return Some(DirectReadMode::Explain);
    }
    None
}

fn path_from_show_verb(text: &str) -> Option<String> {
    let mut tokens = text.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("show") {
        return None;
    }

    let mut candidate = tokens.next()?;
    if candidate.eq_ignore_ascii_case("file") {
        candidate = tokens.next()?;
    }

    let path = candidate.trim_matches(|c: char| {
        matches!(
            c,
            '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    });
    if looks_like_explicit_relative_file_path(path) {
        Some(path.to_string())
    } else {
        None
    }
}

/// Extracts a directory target from navigation prompts.
///
/// Fires when a nav verb is present AND either:
/// - an explicit path token containing `/` is found → returns that path, or
/// - a structural cue word is found with no qualifying path token → returns `"."`
///
/// Returns None when the nav verb is absent, when only a plain non-path word
/// is present (no slash, no structural cue), or when multiple path tokens are
/// found (ambiguous).
fn extract_directory_target(text: &str) -> Option<String> {
    const NAV_VERBS: &[&str] = &["explore", "list", "show", "display", "tree"];
    const STRUCTURAL_CUES: &[&str] = &[
        "files",
        "file",
        "directory",
        "dir",
        "dirs",
        "folder",
        "folders",
        "contents",
    ];

    let tokens = normalized_prompt_tokens(text);
    let has_nav_verb = tokens.iter().any(|t| NAV_VERBS.contains(&t.as_str()));
    if !has_nav_verb {
        return None;
    }

    let mut found_path: Option<String> = None;
    for token in text.split_whitespace() {
        let stripped = token.trim_end_matches(|c: char| {
            matches!(
                c,
                '.' | ',' | '?' | '!' | ';' | ':' | ')' | ']' | '}' | '"' | '\''
            )
        });
        if stripped.is_empty() || !stripped.contains('/') {
            continue;
        }
        if stripped.starts_with("http://") || stripped.starts_with("https://") {
            continue;
        }
        if found_path.is_some() {
            return None; // ambiguous
        }
        found_path = Some(stripped.to_string());
    }

    if let Some(path) = found_path {
        return Some(path);
    }

    let has_structural_cue = tokens.iter().any(|t| STRUCTURAL_CUES.contains(&t.as_str()));
    if has_structural_cue {
        return Some(".".to_string());
    }

    None
}

/// snake_case: contains underscore, ≥2 segments, each segment ≥2 alphanumeric chars.
pub(crate) fn is_snake_case_identifier(token: &str) -> bool {
    if !token.contains('_') {
        return false;
    }
    let segments: Vec<&str> = token.split('_').collect();
    segments.len() >= 2
        && segments
            .iter()
            .all(|s| s.len() >= 2 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Matches PascalCase/camelCase identifiers.
/// Note: also intentionally matches ALLCAPS tokens of sufficient length (e.g., DEBUG, README)
/// for Phase 8.4 structural detection.
pub(crate) fn is_pascal_case_identifier(token: &str) -> bool {
    if token.len() < 5 {
        return false;
    }
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    token[1..].chars().any(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_classifier_accepts_valid_identifiers() {
        assert!(is_snake_case_identifier("run_turns"));
        assert!(is_snake_case_identifier("search_code"));
        assert!(is_snake_case_identifier("read_file"));
        assert!(is_snake_case_identifier("tool_rounds"));
        assert!(is_snake_case_identifier("investigation_state"));
        assert!(is_snake_case_identifier("is_snake_case_identifier"));
    }

    #[test]
    fn snake_case_classifier_rejects_non_identifiers() {
        assert!(!is_snake_case_identifier("word"));
        assert!(!is_snake_case_identifier("a_b"));
        assert!(!is_snake_case_identifier("_leading"));
        assert!(!is_snake_case_identifier("trailing_"));
        assert!(!is_snake_case_identifier("has space"));
        assert!(!is_snake_case_identifier("run_turns()"));
    }

    #[test]
    fn pascal_case_classifier_accepts_valid_identifiers() {
        assert!(is_pascal_case_identifier("AnswerSource"));
        assert!(is_pascal_case_identifier("RuntimeTerminalReason"));
        assert!(is_pascal_case_identifier("InvestigationState"));
        assert!(is_pascal_case_identifier("ToolInput"));
        assert!(is_pascal_case_identifier("SearchBudget"));
    }

    #[test]
    fn pascal_case_classifier_rejects_non_identifiers() {
        assert!(!is_pascal_case_identifier("Hi"));
        assert!(!is_pascal_case_identifier("Short"));
        assert!(!is_pascal_case_identifier("allower"));
        assert!(!is_pascal_case_identifier("Done"));
    }

    #[test]
    fn prompt_requires_investigation_detects_snake_case() {
        assert!(prompt_requires_investigation("What does run_turns do?"));
        assert!(prompt_requires_investigation("Explain search_code to me."));
        assert!(prompt_requires_investigation("Where is read_file defined?"));
    }

    #[test]
    fn prompt_requires_investigation_detects_pascal_case() {
        assert!(prompt_requires_investigation("What is AnswerSource?"));
        assert!(prompt_requires_investigation("Explain InvestigationState"));
        assert!(prompt_requires_investigation(
            "How does RuntimeTerminalReason work?"
        ));
    }

    #[test]
    fn prompt_requires_investigation_detects_natural_language_lookup() {
        assert!(prompt_requires_investigation(
            "Find where logging is initialized"
        ));
        assert!(prompt_requires_investigation(
            "Find where sessions are saved"
        ));
        assert!(prompt_requires_investigation(
            "Where is configuration loaded?"
        ));
        assert!(prompt_requires_investigation("Where are tasks created?"));
        assert!(prompt_requires_investigation(
            "Find where session creation happens"
        ));
    }

    #[test]
    fn prompt_requires_investigation_detects_rendered_lookup() {
        assert!(prompt_requires_investigation(
            "where is git status rendered"
        ));
        assert!(prompt_requires_investigation(
            "Find where status is rendered"
        ));
    }

    #[test]
    fn prompt_requires_investigation_detects_search_verb() {
        assert!(prompt_requires_investigation(
            "Search for 'task' in sandbox/ and explain what parts of the system use it."
        ));
        assert!(prompt_requires_investigation("Search for task in sandbox/"));
        assert!(prompt_requires_investigation(
            "search the codebase for SessionLog"
        ));
    }

    #[test]
    fn prompt_requires_investigation_detects_occurrence_phrasing() {
        assert!(prompt_requires_investigation(
            "Find all occurrences of 'logging' in sandbox/ and summarize where it appears."
        ));
        assert!(prompt_requires_investigation(
            "Find all occurrences of logging in sandbox/"
        ));
        assert!(prompt_requires_investigation(
            "Where does TaskStatus appear in the codebase?"
        ));
        assert!(prompt_requires_investigation(
            "Find where the error occurs in this module."
        ));
        assert!(prompt_requires_investigation(
            "Where are completed tasks filtered in sandbox/"
        ));
    }

    #[test]
    fn prompt_requires_investigation_rejects_plain_questions() {
        assert!(!prompt_requires_investigation("How are you?"));
        assert!(!prompt_requires_investigation("What time is it?"));
        assert!(!prompt_requires_investigation(
            "Can you help me with something?"
        ));
        assert!(!prompt_requires_investigation(
            "What is the purpose of this project?"
        ));
        assert!(!prompt_requires_investigation(
            "Find a good approach to this problem."
        ));
        assert!(!prompt_requires_investigation(
            "Find the best way to structure this."
        ));
    }

    #[test]
    fn mutation_intent_classifier_ignores_tool_name_mentions() {
        assert!(!user_requested_mutation("Where is write_file implemented?"));
        assert!(!user_requested_mutation("How does edit_file recover?"));
        assert!(user_requested_mutation("Create a file named demo.txt"));
        assert!(user_requested_mutation(
            "Edit src/main.rs and change hello to hi"
        ));
    }

    #[test]
    fn looks_like_file_path_accepts_real_paths() {
        assert!(looks_like_file_path("src/tools/mod.rs"));
        assert!(looks_like_file_path("README"));
        assert!(looks_like_file_path("readme"));
        assert!(looks_like_file_path("Cargo.toml"));
        assert!(looks_like_file_path("task_service.py"));
        assert!(looks_like_file_path(".gitignore"));
        assert!(looks_like_file_path("src\\tui\\app.rs"));
        assert!(looks_like_file_path("sandbox/"));
        assert!(looks_like_file_path("main.rs"));
    }

    #[test]
    fn looks_like_file_path_rejects_natural_language() {
        // Incident 6 repro: NL agent target containing a trailing-slash token.
        assert!(!looks_like_file_path(
            "Explain the purpose of the sandbox/ project"
        ));
        assert!(!looks_like_file_path("how does the sandbox/ project work"));
        assert!(!looks_like_file_path(
            "what does main.rs do in context of the project"
        ));
        assert!(!looks_like_file_path("compare v1.2 and v2.0 behavior"));
        assert!(!looks_like_file_path("did it work?"));
        assert!(!looks_like_file_path("ship it!"));
        assert!(!looks_like_file_path("done."));
        assert!(!looks_like_file_path("e.g."));
        assert!(!looks_like_file_path(""));
    }

    #[test]
    fn looks_like_file_path_known_residual_nl_fragments() {
        // Accepted residual risk (documented on the function): whitespace-free
        // NL fragments with a separator or extension shape still pass.
        assert!(looks_like_file_path("either/or"));
        assert!(looks_like_file_path("v1.2"));
    }

    #[test]
    fn requested_read_path_detects_explicit_file_reads() {
        assert_eq!(
            requested_read_path("Read missing_file_phase84x.rs").as_deref(),
            Some("missing_file_phase84x.rs")
        );
        assert_eq!(
            requested_read_path("read file `src/runtime/engine.rs`").as_deref(),
            Some("src/runtime/engine.rs")
        );
        assert_eq!(requested_read_path("Read about logging"), None);
    }

    #[test]
    fn requested_read_path_detects_path_qualified_what_is_in_query() {
        assert_eq!(
            requested_read_path("What is in src/runtime/engine.rs?").as_deref(),
            Some("src/runtime/engine.rs")
        );
        assert_eq!(
            requested_read_path("What is in the sandbox/main.py?").as_deref(),
            Some("sandbox/main.py")
        );
        // bare filename (no slash) must NOT become a direct-read
        assert_eq!(
            requested_read_path("What is in engine.rs?").as_deref(),
            None
        );
        // non-file query must not match
        assert_eq!(
            requested_read_path("What is in this project?").as_deref(),
            None
        );
    }

    #[test]
    fn classify_retrieval_intent_distinguishes_raw_and_explain_direct_reads() {
        assert!(matches!(
            classify_retrieval_intent("What does sandbox/services/task_service.py do?"),
            RetrievalIntent::DirectRead { path, mode: DirectReadMode::Explain }
                if path == "sandbox/services/task_service.py"
        ));
        assert!(matches!(
            classify_retrieval_intent("Explain sandbox/services/task_service.py"),
            RetrievalIntent::DirectRead { path, mode: DirectReadMode::Explain }
                if path == "sandbox/services/task_service.py"
        ));
        assert!(matches!(
            classify_retrieval_intent("Read sandbox/services/task_service.py"),
            RetrievalIntent::DirectRead { path, mode: DirectReadMode::Raw }
                if path == "sandbox/services/task_service.py"
        ));
        assert!(matches!(
            classify_retrieval_intent("Show sandbox/services/task_service.py"),
            RetrievalIntent::DirectRead { path, mode: DirectReadMode::Raw }
                if path == "sandbox/services/task_service.py"
        ));
        assert!(matches!(
            classify_retrieval_intent("What is in sandbox/services/task_service.py?"),
            RetrievalIntent::DirectRead { path, mode: DirectReadMode::Raw }
                if path == "sandbox/services/task_service.py"
        ));
        assert!(!matches!(
            classify_retrieval_intent("Where are completed tasks filtered in sandbox/"),
            RetrievalIntent::DirectRead { .. }
        ));
    }

    #[test]
    fn requested_simple_edit_detects_long_form() {
        let edit = requested_simple_edit(
            "Edit the file test.txt replace the content hello world with hello thunk",
        )
        .expect("expected simple edit");
        assert_eq!(edit.path, "test.txt");
        assert_eq!(edit.search, "hello world");
        assert_eq!(edit.replace, "hello thunk");
    }

    #[test]
    fn requested_simple_edit_detects_short_form() {
        let edit = requested_simple_edit("Edit hello.txt replace hello root with hello runtime")
            .expect("expected simple edit");
        assert_eq!(edit.path, "hello.txt");
        assert_eq!(edit.search, "hello root");
        assert_eq!(edit.replace, "hello runtime");
    }

    #[test]
    fn requested_simple_edit_detects_and_change_form() {
        let edit =
            requested_simple_edit("Edit baseline_test.txt and change hello world to hello thunk")
                .expect("expected simple edit");
        assert_eq!(edit.path, "baseline_test.txt");
        assert_eq!(edit.search, "hello world");
        assert_eq!(edit.replace, "hello thunk");
    }

    #[test]
    fn requested_simple_edit_detects_bare_change_form() {
        let edit =
            requested_simple_edit("Edit src/config.rs change default_timeout to request_timeout")
                .expect("expected simple edit");
        assert_eq!(edit.path, "src/config.rs");
        assert_eq!(edit.search, "default_timeout");
        assert_eq!(edit.replace, "request_timeout");
    }

    #[test]
    fn requested_simple_edit_detects_edit_the_file_change_form() {
        let edit = requested_simple_edit(
            "Edit the file baseline_test.txt change hello world to hello thunk",
        )
        .expect("expected simple edit");
        assert_eq!(edit.path, "baseline_test.txt");
        assert_eq!(edit.search, "hello world");
        assert_eq!(edit.replace, "hello thunk");
    }

    #[test]
    fn requested_simple_edit_detects_to_change_form() {
        let edit = requested_simple_edit("Edit config.txt to change old_value to new_value")
            .expect("expected simple edit");
        assert_eq!(edit.path, "config.txt");
        assert_eq!(edit.search, "old_value");
        assert_eq!(edit.replace, "new_value");
    }

    #[test]
    fn requested_simple_edit_detects_in_path_change_form() {
        let edit = requested_simple_edit("In notes.txt change draft to final")
            .expect("expected simple edit");
        assert_eq!(edit.path, "notes.txt");
        assert_eq!(edit.search, "draft");
        assert_eq!(edit.replace, "final");
    }

    #[test]
    fn prompt_requires_investigation_detects_bare_filename_tokens() {
        assert!(prompt_requires_investigation("What is in engine.rs?"));
        assert!(prompt_requires_investigation("What does main.py do?"));
        assert!(prompt_requires_investigation("Explain tool_surface.rs"));
        assert!(prompt_requires_investigation("Show me Cargo.toml"));
    }

    #[test]
    fn prompt_requires_investigation_rejects_non_code_extension_tokens() {
        // Version strings and other numeric dot-separated tokens must not trigger.
        assert!(!prompt_requires_investigation("Python 3.10 syntax is fine"));
        assert!(!prompt_requires_investigation("version 1.0 released"));
    }

    #[test]
    fn prompt_requires_investigation_detects_where_is_code_noun() {
        assert!(prompt_requires_investigation(
            "Where is the helper function?"
        ));
        assert!(prompt_requires_investigation("Where is the config module?"));
        assert!(prompt_requires_investigation("Where is the parser file?"));
        assert!(prompt_requires_investigation("Where is the command?"));
        assert!(prompt_requires_investigation("Where is the main class?"));
    }

    #[test]
    fn prompt_requires_investigation_rejects_where_is_non_code_noun() {
        assert!(!prompt_requires_investigation(
            "Where is the best place to start?"
        ));
        assert!(!prompt_requires_investigation("Where is the issue?"));
        assert!(!prompt_requires_investigation(
            "Where is the project summary?"
        ));
    }

    #[test]
    fn prompt_requires_investigation_detects_cross_layer_traversal() {
        assert!(prompt_requires_investigation(
            "Find how the CLI dispatch layer reaches the storage layer in sandbox/"
        ));
        assert!(prompt_requires_investigation(
            "Find how the task service interacts with the repository in sandbox/"
        ));
        assert!(prompt_requires_investigation(
            "trace how the request reaches the backend"
        ));
        assert!(prompt_requires_investigation(
            "follow how the command connects to the handler"
        ));
    }

    #[test]
    fn prompt_requires_investigation_rejects_non_code_reach_phrasing() {
        assert!(!prompt_requires_investigation("trace the logs"));
        assert!(!prompt_requires_investigation("follow the instructions"));
        assert!(!prompt_requires_investigation("follow up on the PR"));
    }

    #[test]
    fn extract_investigation_path_scope_detects_in_pattern() {
        assert_eq!(
            extract_investigation_path_scope("Where is TaskStatus handled in sandbox/cli/"),
            Some("sandbox/cli/".into())
        );
        assert_eq!(
            extract_investigation_path_scope(
                "Find where logging is initialized in sandbox/services/"
            ),
            Some("sandbox/services/".into())
        );
        assert_eq!(
            extract_investigation_path_scope("Where is TaskStatus used in sandbox/"),
            Some("sandbox/".into())
        );
    }

    #[test]
    fn extract_investigation_path_scope_detects_the_before_path() {
        assert_eq!(
            extract_investigation_path_scope(
                "Find where database is configured in the sandbox/ folder"
            ),
            Some("sandbox/".into())
        );
    }

    #[test]
    fn extract_investigation_path_scope_detects_within_pattern() {
        assert_eq!(
            extract_investigation_path_scope("Find TaskStatus within sandbox/cli/"),
            Some("sandbox/cli/".into())
        );
    }

    #[test]
    fn extract_investigation_path_scope_rejects_plain_words() {
        assert_eq!(
            extract_investigation_path_scope("Find X in the application"),
            None
        );
        assert_eq!(
            extract_investigation_path_scope("Where is X used in context"),
            None
        );
        assert_eq!(
            extract_investigation_path_scope("What does run_turns do?"),
            None
        );
    }

    #[test]
    fn extract_investigation_path_scope_rejects_urls() {
        assert_eq!(
            extract_investigation_path_scope("Find X in https://example.com/path"),
            None
        );
    }

    #[test]
    fn extract_investigation_path_scope_returns_none_for_ambiguous_multiple_paths() {
        assert_eq!(
            extract_investigation_path_scope("Find X in sandbox/a/ and in sandbox/b/"),
            None
        );
    }

    #[test]
    fn extract_investigation_path_scope_strips_trailing_punctuation() {
        assert_eq!(
            extract_investigation_path_scope("Where is TaskStatus in sandbox/cli?"),
            Some("sandbox/cli".into())
        );
        assert_eq!(
            extract_investigation_path_scope("Find X in sandbox/services/."),
            Some("sandbox/services/".into())
        );
    }

    #[test]
    fn path_from_bare_filename_explain_prompt_fires_on_explanation_verbs() {
        assert_eq!(
            path_from_bare_filename_explain_prompt("What does task_service.py do?"),
            Some("task_service.py".into())
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Explain engine.rs"),
            Some("engine.rs".into())
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Describe config.toml please"),
            Some("config.toml".into())
        );
    }

    #[test]
    fn path_from_bare_filename_explain_prompt_rejects_path_qualified_tokens() {
        assert_eq!(
            path_from_bare_filename_explain_prompt(
                "What does sandbox/services/task_service.py do?"
            ),
            None
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Explain src/runtime/engine.rs"),
            None
        );
    }

    #[test]
    fn path_from_bare_filename_explain_prompt_rejects_non_explanation_verbs() {
        assert_eq!(
            path_from_bare_filename_explain_prompt("Find task_service.py in the codebase"),
            None
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Where is task_service.py used?"),
            None
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Read task_service.py"),
            None
        );
    }

    #[test]
    fn path_from_bare_filename_explain_prompt_returns_none_for_multiple_filenames() {
        assert_eq!(
            path_from_bare_filename_explain_prompt(
                "What does task_service.py and user_service.py do?"
            ),
            None
        );
    }

    #[test]
    fn path_from_bare_filename_explain_prompt_rejects_non_code_extensions() {
        assert_eq!(
            path_from_bare_filename_explain_prompt("What does version 3.14 mean?"),
            None
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Explain v1.2 syntax"),
            None
        );
    }

    #[test]
    fn path_from_bare_filename_explain_prompt_strips_trailing_punctuation() {
        assert_eq!(
            path_from_bare_filename_explain_prompt("What does engine.rs?"),
            Some("engine.rs".into())
        );
        assert_eq!(
            path_from_bare_filename_explain_prompt("Explain main.py!"),
            Some("main.py".into())
        );
    }

    #[test]
    fn requested_read_path_detects_bare_filename_explain_prompts() {
        assert_eq!(
            requested_read_path("What does task_service.py do?").as_deref(),
            Some("task_service.py")
        );
        assert_eq!(
            requested_read_path("Explain engine.rs").as_deref(),
            Some("engine.rs")
        );
        assert_eq!(
            requested_read_path("Describe config.toml").as_deref(),
            Some("config.toml")
        );
        // path-qualified form still handled by earlier arm
        assert_eq!(
            requested_read_path("What does sandbox/services/task_service.py do?").as_deref(),
            Some("sandbox/services/task_service.py")
        );
        // ambiguous — two filenames
        assert_eq!(
            requested_read_path("What does task_service.py and user_service.py do?").as_deref(),
            None
        );
    }

    #[test]
    fn requested_read_path_find_what_bare_filename() {
        assert_eq!(
            requested_read_path("Find what task_service.py does").as_deref(),
            Some("task_service.py")
        );
    }

    #[test]
    fn requested_read_path_find_what_path_qualified() {
        assert_eq!(
            requested_read_path("Find what sandbox/services/task_service.py does").as_deref(),
            Some("sandbox/services/task_service.py")
        );
    }

    #[test]
    fn requested_read_path_find_what_no_file_token_returns_none() {
        assert_eq!(
            requested_read_path("Find what the project does").as_deref(),
            None
        );
    }

    #[test]
    fn is_permitted_shell_command_allows_cargo() {
        assert!(is_permitted_shell_command("cargo check"));
        assert!(is_permitted_shell_command("cargo test my_filter"));
        assert!(is_permitted_shell_command("cargo clippy"));
        assert!(is_permitted_shell_command("cargo"));
    }

    #[test]
    fn is_permitted_shell_command_rejects_unknown() {
        assert!(!is_permitted_shell_command("npm install"));
        assert!(!is_permitted_shell_command("make build"));
        assert!(!is_permitted_shell_command("python main.py"));
    }

    #[test]
    fn is_permitted_shell_command_rejects_empty() {
        assert!(!is_permitted_shell_command(""));
        assert!(!is_permitted_shell_command("   "));
    }

    #[test]
    fn requested_shell_command_strips_shell_command_preamble() {
        assert_eq!(
            requested_shell_command("run the shell command: grep -r foo src/"),
            Some("grep -r foo src/".to_string())
        );
        assert_eq!(
            requested_shell_command("execute command: ls -la"),
            Some("ls -la".to_string())
        );
        assert_eq!(
            requested_shell_command("run shell command: cat Cargo.toml"),
            Some("cat Cargo.toml".to_string())
        );
    }

    #[test]
    fn requested_shell_command_without_preamble_is_unchanged() {
        assert_eq!(
            requested_shell_command("run cargo test"),
            Some("cargo test".to_string())
        );
        assert_eq!(
            requested_shell_command("execute mkdir foo"),
            Some("mkdir foo".to_string())
        );
    }

    #[test]
    fn user_requested_remember_apostrophe_less_form() {
        assert!(user_requested_remember("dont forget my API key is abc123").is_some());
        assert!(user_requested_remember("dont forget that the deadline is Friday").is_some());
        assert_eq!(
            user_requested_remember("dont forget my API key is abc123"),
            Some("my API key is abc123".to_string())
        );
        assert_eq!(
            user_requested_remember("dont forget that the deadline is Friday"),
            Some("the deadline is Friday".to_string())
        );
    }
}
