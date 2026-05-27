use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::tools::ToolOutput;

use super::graph::InvestigationGraph;
use super::super::paths::normalize_evidence_path;
use super::super::types::RuntimeEvent;

const RUNTIME_TRACE_ENV: &str = "THUNK_TRACE_RUNTIME";

// Exact substring triggers used for structured investigation modes.
// Keep these narrow: broad matching increases false positives for small local models.
const INITIALIZATION_TERMS: &[&str] = &["initialize", "initialized", "initialization"];
const CREATE_TERMS: &[&str] = &["create", "created", "creation"];
const REGISTER_TERMS: &[&str] = &["register", "registered", "registration"];
const LOAD_TERMS: &[&str] = &["load", "loaded", "loading"];
const SAVE_TERMS: &[&str] = &["save", "saved", "saving"];

// Lockfiles are useful project metadata, but usually poor evidence for code-location answers.
const LOCKFILE_NAMES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
    "Pipfile.lock",
];

// Source extensions used to prefer implementation files over generated or metadata matches.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "cpp", "h", "hpp",
];

// Advisory runtime tracing only. Trace events must not influence control flow.
fn trace_runtime_decision(
    on_event: &mut dyn FnMut(RuntimeEvent),
    event: &str,
    fields: &[(&str, String)],
) {
    if std::env::var_os(RUNTIME_TRACE_ENV).is_none() {
        return;
    }

    let mut line = format!("[runtime:trace] event={event}");
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&trace_field_value(value));
    }
    on_event(RuntimeEvent::RuntimeTrace(line));
}

fn trace_field_value(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("{value:?}")
    }
}

fn push_unique_path(paths: &mut Vec<String>, path: &str) {
    if !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_string());
    }
}

pub(crate) fn contains_initialization_term(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    INITIALIZATION_TERMS.iter().any(|term| lower.contains(term))
}

pub(crate) fn contains_create_term(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    CREATE_TERMS.iter().any(|term| lower.contains(term))
}

pub(crate) fn contains_register_term(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    REGISTER_TERMS.iter().any(|term| lower.contains(term))
}

pub(crate) fn contains_load_term(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    LOAD_TERMS.iter().any(|term| lower.contains(term))
}

pub(crate) fn contains_save_term(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    SAVE_TERMS.iter().any(|term| lower.contains(term))
}

fn contains_word(text: &str, needle: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|token| token == needle)
}

/// Returns true if the path's file extension identifies it as a config file.
/// Classification is purely extension-based — no content analysis or filename heuristics.
/// Handles the exact `.env` dotfile explicitly since `Path::extension()` returns None for it.
pub(crate) fn is_config_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let p = Path::new(&lower);
    if matches!(
        p.extension().and_then(|e| e.to_str()),
        Some("yaml" | "yml" | "toml" | "json" | "ini" | "cfg" | "conf" | "properties")
    ) {
        return true;
    }
    // `.env` is part of the allowed config extension set; `.env.*` is intentionally
    // excluded because its actual extension is something else.
    if let Some(filename) = p.file_name().and_then(|f| f.to_str()) {
        if filename == ".env" {
            return true;
        }
    }
    false
}

fn is_lockfile_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .is_some_and(|filename| LOCKFILE_NAMES.contains(&filename))
}

fn is_source_candidate_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| SOURCE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Returns true if the line (after stripping leading whitespace) is an import declaration.
/// Coverage: Python (`import X`, `from X import Y`) and Java/Go/TypeScript (`import X`).
/// Rust `use` statements and C `#include` are intentionally excluded — too many false positives
/// from identifiers like `use` appearing in natural language or in assertion-style code.
/// No regex, no scoring — prefix matching only, same style as looks_like_definition.
pub(crate) fn looks_like_import(line: &str) -> bool {
    let t = line.trim_start();
    // `import X` — Python, Java, Go, TypeScript, JavaScript
    t.starts_with("import ")
        // `from X import Y` — Python
        || (t.starts_with("from ") && t.contains(" import "))
}

/// Returns true if the line defines the exact identifier `symbol`.
/// Strips each known definition prefix, extracts the first alphanumeric+underscore token,
/// and requires exact equality — so "class TaskStatus:" does not match symbol "Task".
/// Coverage mirrors `looks_like_definition`.
pub(crate) fn looks_like_definition_of_symbol(line: &str, symbol: &str) -> bool {
    let t = line.trim_start();
    const PREFIXES: &[&str] = &[
        "pub enum ",
        "pub struct ",
        "pub fn ",
        "pub type ",
        "pub trait ",
        "pub const ",
        "pub static ",
        "enum ",
        "struct ",
        "fn ",
        "type ",
        "const ",
        "trait ",
        "impl ",
        "class ",
        "def ",
        "func ",
        "function ",
        "interface ",
    ];
    for prefix in PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            let ident = rest
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("");
            if ident == symbol {
                return true;
            }
        }
    }
    false
}

/// Returns true if the line contains a call expression for the exact identifier `symbol`.
/// Detection: `symbol(` anywhere on the line, excluding lines that define the symbol.
/// Covers direct calls (`symbol(args)`) and method calls (`.symbol(args)`).
/// No regex — substring matching only.
pub(crate) fn looks_like_call_expression_of_symbol(line: &str, symbol: &str) -> bool {
    if looks_like_definition_of_symbol(line, symbol) {
        return false;
    }
    line.contains(&format!("{symbol}("))
}

fn looks_like_call_expression(line: &str) -> bool {
    !looks_like_definition(line) && line.contains('(')
}

/// Returns true if the line (after stripping leading whitespace) looks like a symbol definition.
/// Coverage: Rust, Python, Go, TypeScript, JavaScript.
/// C/C++ patterns are excluded — too many false positives without a type parser.
/// No regex, no scoring — prefix matching only.
fn looks_like_definition(line: &str) -> bool {
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

/// Structural mode for the current investigation turn.
/// Computed once from the user prompt before the tool loop starts.
/// Controls which evidence-acceptance gates are active for this turn.
#[derive(Copy, Clone)]
pub(crate) enum InvestigationMode {
    /// No mode-specific gating. Any search-candidate read satisfies evidence.
    General,
    /// Prompt signals a call-site lookup (where X is called/invoked/used by).
    /// Non-call-site reads are structurally insufficient when call-site candidates exist.
    CallSiteLookup,
    /// Prompt signals a usage lookup (where X is used/referenced/appears).
    /// Definition-only reads are structurally insufficient when usage candidates exist.
    UsageLookup,
    /// Prompt signals a definition lookup (where X is defined/declared).
    /// No mode-specific gating beyond General — definition reads are always accepted.
    DefinitionLookup,
    /// Prompt signals a config lookup (where X is configured/configuration).
    /// Source-file reads are structurally insufficient when config-file candidates exist.
    ConfigLookup,
    /// Prompt signals a narrow initialization lookup.
    /// Non-initialization reads are structurally insufficient when initialization candidates exist.
    InitializationLookup,
    /// Prompt signals a narrow creation lookup (where X is created/creation).
    /// Non-create reads are structurally insufficient when create candidates exist.
    CreateLookup,
    /// Prompt signals a narrow registration lookup.
    /// Non-register reads are structurally insufficient when register candidates exist.
    RegisterLookup,
    /// Prompt signals a narrow load lookup.
    /// Non-load reads are structurally insufficient when load candidates exist.
    LoadLookup,
    /// Prompt signals a narrow save lookup.
    /// Non-save reads are structurally insufficient when save candidates exist.
    SaveLookup,
}

impl InvestigationMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            InvestigationMode::General => "General",
            InvestigationMode::CallSiteLookup => "CallSiteLookup",
            InvestigationMode::UsageLookup => "UsageLookup",
            InvestigationMode::DefinitionLookup => "DefinitionLookup",
            InvestigationMode::ConfigLookup => "ConfigLookup",
            InvestigationMode::InitializationLookup => "InitializationLookup",
            InvestigationMode::CreateLookup => "CreateLookup",
            InvestigationMode::RegisterLookup => "RegisterLookup",
            InvestigationMode::LoadLookup => "LoadLookup",
            InvestigationMode::SaveLookup => "SaveLookup",
        }
    }
}

/// Detects the structural investigation mode from the prompt text.
/// Evaluated in priority order so each prompt maps to exactly one mode.
/// Priority: CallSiteLookup > UsageLookup > ConfigLookup > InitializationLookup > CreateLookup > RegisterLookup > LoadLookup > SaveLookup > DefinitionLookup > General.
pub(crate) fn detect_investigation_mode(text: &str) -> InvestigationMode {
    let lower = text.to_ascii_lowercase();
    if ["called", "invoked", "calls", "invoke", "invocation"]
        .iter()
        .any(|term| contains_word(&lower, term))
        || lower.contains("used by")
    {
        return InvestigationMode::CallSiteLookup;
    }
    if [
        "use",
        "used",
        "uses",
        "usage",
        "reference",
        "referenced",
        "references",
        "occur",
        "occurs",
        "occurrence",
        "occurrences",
        "appear",
        "appears",
    ]
    .iter()
    .any(|term| contains_word(&lower, term))
    {
        return InvestigationMode::UsageLookup;
    }
    if ["config", "configured", "configuration", "configure"]
        .iter()
        .any(|term| contains_word(&lower, term))
    {
        return InvestigationMode::ConfigLookup;
    }
    if contains_initialization_term(&lower) {
        return InvestigationMode::InitializationLookup;
    }
    if contains_create_term(&lower) {
        return InvestigationMode::CreateLookup;
    }
    if contains_register_term(&lower) {
        return InvestigationMode::RegisterLookup;
    }
    if contains_load_term(&lower) {
        return InvestigationMode::LoadLookup;
    }
    if contains_save_term(&lower) {
        return InvestigationMode::SaveLookup;
    }
    if [
        "defined",
        "definition",
        "declared",
        "declares",
        "declaration",
    ]
    .iter()
    .any(|term| contains_word(&lower, term))
    {
        return InvestigationMode::DefinitionLookup;
    }
    InvestigationMode::General
}

/// Distinguishes which structural insufficiency caused a candidate read to be rejected.
/// Used by the caller in run_tool_round to select the appropriate correction message.
pub(crate) enum RecoveryKind {
    /// The file was definition-only on a usage lookup with usage candidates available.
    DefinitionOnly,
    /// The file was not a definition-site candidate on a definition lookup when definition
    /// candidates exist. Runtime dispatches the correct definition file directly.
    NonDefinitionSite,
    /// The file had only import-declaration matches with substantive candidates available.
    ImportOnly,
    /// The file was a non-config source file on a config lookup when config-file candidates exist.
    ConfigFile,
    /// The file lacked initialization matches when initialization candidates exist.
    Initialization,
    /// The file lacked create-term matches when create candidates exist.
    Create,
    /// The file lacked register-term matches when register candidates exist.
    Register,
    /// The file lacked call-expression matches when call-site candidates exist.
    CallSite,
    /// The file lacked load-term matches when load candidates exist.
    Load,
    /// The file had load-term matches only on definition lines when call-site load candidates exist.
    LoadDefinitionOnly,
    /// The file lacked save-term matches when save candidates exist.
    Save,
    /// The file was a lockfile when a matched source candidate exists.
    Lockfile,
}

impl RecoveryKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            RecoveryKind::DefinitionOnly => "DefinitionOnly",
            RecoveryKind::NonDefinitionSite => "NonDefinitionSite",
            RecoveryKind::ImportOnly => "ImportOnly",
            RecoveryKind::ConfigFile => "ConfigFile",
            RecoveryKind::Initialization => "Initialization",
            RecoveryKind::Create => "Create",
            RecoveryKind::Register => "Register",
            RecoveryKind::CallSite => "CallSite",
            RecoveryKind::Load => "Load",
            RecoveryKind::LoadDefinitionOnly => "LoadDefinitionOnly",
            RecoveryKind::Save => "Save",
            RecoveryKind::Lockfile => "Lockfile",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadClassification {
    Direct,
    Candidate,
}

/// Tracks per-turn search → read investigation state.
/// Resets at the start of each call to run_turns, exactly like SearchBudget.
pub(crate) struct InvestigationState {
    /// True once any search_code call this turn returned at least one match.
    search_produced_results: bool,
    /// Count of read_file calls that completed successfully this turn.
    files_read_count: usize,
    /// File paths from the current non-empty search results.
    search_candidate_paths: Vec<String>,
    /// Candidate paths where every matched line looks like a definition site.
    /// Populated during record_search_results alongside search_candidate_paths.
    pub(crate) definition_only_candidates: HashSet<String>,
    /// Count of matched lines per candidate that are not definition sites.
    /// Preserves only search-result-local evidence and is used for UsageLookup
    /// candidate quality ranking after search_code succeeds.
    non_definition_match_counts: HashMap<String, usize>,
    /// Candidate paths where at least one matched line is an exact definition of the
    /// queried symbol (weaker than definition_only_candidates, which requires all lines).
    /// Used exclusively by DefinitionLookup: Gate 8 and first_definition_candidate.
    definition_site_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has a
    /// non-definition match line (i.e. a usage file is available).
    pub(crate) has_non_definition_candidates: bool,
    /// Number of accepted matched-candidate reads that counted as useful evidence.
    /// Kept separate from candidate_reads_count so the runtime can distinguish
    /// broad UsageLookup happy-path reads from rejected or fallback reads.
    pub(crate) useful_accepted_candidate_reads: usize,
    /// Normalized paths of accepted matched-candidate reads that counted as useful evidence.
    /// Used to deterministically exclude already-read candidates when broad UsageLookup
    /// requires a second runtime-owned evidence read.
    useful_accepted_candidate_paths: HashSet<String>,
    /// True after the read-before-answering correction has been issued once.
    /// Prevents the correction from firing more than once per turn.
    premature_synthesis_correction_issued: bool,
    /// True after the search-before-answering correction has been issued once.
    /// R1 uses its own flag and does NOT increment the shared corrections counter,
    /// so R1 and R2 can compose sequentially in the same turn.
    direct_answer_correction_issued: bool,
    /// True once any search_code call has completed this turn, even with no matches.
    /// Used to block list_dir before search on investigation-required turns.
    search_attempted: bool,
    /// Count of distinct search-candidate files successfully read this turn.
    /// Bounded investigation: a second candidate read is allowed when the first was
    /// insufficient; after two candidate reads the runtime terminates cleanly if
    /// evidence_ready() is still false.
    pub(crate) candidate_reads_count: usize,
    pub(crate) direct_reads_count: usize,
    pub(crate) direct_read_paths: HashSet<String>,
    /// True when this turn is a broad UsageLookup prompt eligible for the
    /// multi-candidate evidence policy.
    broad_usage_lookup: bool,
    /// Runtime-owned target for how many accepted useful matched-candidate reads
    /// are required before evidence is ready. Defaults to 1; broad UsageLookup
    /// may raise it to 2 when two substantive candidates are available.
    useful_candidate_reads_target: usize,
    /// Candidate paths where every matched line looks like an import declaration.
    /// Populated during record_search_results alongside search_candidate_paths.
    import_only_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has a non-import
    /// match line (i.e. a file with substantive usage or definition is available).
    has_non_import_candidates: bool,
    /// True after the import-only recovery correction has been issued once this turn.
    /// Uses its own flag so it does not consume the premature_synthesis correction slot.
    import_correction_issued: bool,
    /// Candidate paths whose file extension identifies them as a config file
    /// (e.g. .yaml, .toml, .json, .env).  Populated during record_search_results.
    config_file_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results is NOT a config file
    /// (i.e. a source or other non-config file was also matched).
    has_non_config_candidates: bool,
    /// True after the config-file recovery correction has been issued once this turn.
    config_correction_issued: bool,
    /// Candidate paths where at least one matched line contains an initialization term.
    /// Populated during record_search_results alongside search_candidate_paths.
    initialization_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has no matched
    /// initialization line.
    has_non_initialization_candidates: bool,
    /// True after the initialization recovery correction has been issued once this turn.
    initialization_correction_issued: bool,
    /// Candidate paths where at least one matched line contains a create term.
    /// Populated during record_search_results alongside search_candidate_paths.
    create_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has no matched
    /// create line.
    has_non_create_candidates: bool,
    /// True after the create recovery correction has been issued once this turn.
    create_correction_issued: bool,
    /// Candidate paths where at least one matched line contains a register term.
    /// Populated during record_search_results alongside search_candidate_paths.
    register_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has no matched
    /// register line.
    has_non_register_candidates: bool,
    /// True after the register recovery correction has been issued once this turn.
    register_correction_issued: bool,
    /// Candidate paths where at least one matched line contains a call expression.
    /// Populated during record_search_results alongside search_candidate_paths.
    pub(crate) call_site_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has no call-expression
    /// match line (i.e. a definition-only or non-call file is available alongside a call-site file).
    has_non_call_site_candidates: bool,
    /// True after the call-site recovery correction has been issued once this turn.
    call_site_correction_issued: bool,
    /// Candidate paths where at least one matched line contains a load term.
    /// Populated during record_search_results alongside search_candidate_paths.
    load_candidates: HashSet<String>,
    /// True if at least one candidate in the current search results has no matched
    /// load line.
    has_non_load_candidates: bool,
    /// True after the load recovery correction has been issued once this turn.
    load_correction_issued: bool,
    /// Candidate paths in load_candidates where every load-term matched line is also a
    /// definition site. Populated during record_search_results alongside load_candidates.
    load_definition_only_candidates: HashSet<String>,
    /// True if at least one load candidate has a load-term match on a non-definition line.
    has_non_definition_load_candidates: bool,
    /// True after the load-definition-only recovery correction has been issued once this turn.
    load_definition_only_correction_issued: bool,
    /// Candidate paths where at least one matched line contains a save term.
    /// Populated during record_search_results alongside search_candidate_paths.
    save_candidates: HashSet<String>,
    /// True after the save recovery correction has been issued once this turn.
    save_correction_issued: bool,
    /// Candidate paths whose basename is an exact known lockfile name.
    lockfile_candidates: HashSet<String>,
    /// True after the lockfile recovery correction has been issued once this turn.
    lockfile_correction_issued: bool,
    /// Number of times a non-candidate read_file was attempted this turn.
    /// Persists across run_tool_round calls so the repeated-offense terminal fires
    /// even when the first offense and second offense are in separate model responses.
    non_candidate_read_attempts: usize,
    /// Summaries of accepted search calls this turn, for evidence citation on approval.
    accepted_search_summaries: Vec<String>,
    /// Path dispatched as a definition-site read after usage candidates were exhausted.
    /// When set, Gate 1 is bypassed for this path so the read is accepted as evidence.
    definition_site_dispatch_issued: Option<String>,
    /// Graph-shaped candidate tracker. Records import edges from read files and surfaces
    /// unread imported files as promoted candidates after search candidates are exhausted.
    pub(crate) graph: InvestigationGraph,
}

impl InvestigationState {
    pub(crate) fn new() -> Self {
        Self {
            search_produced_results: false,
            files_read_count: 0,
            search_candidate_paths: Vec::new(),
            definition_only_candidates: HashSet::new(),
            non_definition_match_counts: HashMap::new(),
            definition_site_candidates: HashSet::new(),
            has_non_definition_candidates: false,
            useful_accepted_candidate_reads: 0,
            useful_accepted_candidate_paths: HashSet::new(),
            premature_synthesis_correction_issued: false,
            direct_answer_correction_issued: false,
            search_attempted: false,
            candidate_reads_count: 0,
            broad_usage_lookup: false,
            useful_candidate_reads_target: 1,
            import_only_candidates: HashSet::new(),
            has_non_import_candidates: false,
            import_correction_issued: false,
            config_file_candidates: HashSet::new(),
            has_non_config_candidates: false,
            config_correction_issued: false,
            initialization_candidates: HashSet::new(),
            has_non_initialization_candidates: false,
            initialization_correction_issued: false,
            create_candidates: HashSet::new(),
            has_non_create_candidates: false,
            create_correction_issued: false,
            register_candidates: HashSet::new(),
            has_non_register_candidates: false,
            register_correction_issued: false,
            call_site_candidates: HashSet::new(),
            has_non_call_site_candidates: false,
            call_site_correction_issued: false,
            load_candidates: HashSet::new(),
            has_non_load_candidates: false,
            load_correction_issued: false,
            load_definition_only_candidates: HashSet::new(),
            has_non_definition_load_candidates: false,
            load_definition_only_correction_issued: false,
            save_candidates: HashSet::new(),
            save_correction_issued: false,
            lockfile_candidates: HashSet::new(),
            lockfile_correction_issued: false,
            non_candidate_read_attempts: 0,
            direct_reads_count: 0,
            direct_read_paths: HashSet::new(),
            accepted_search_summaries: vec![],
            definition_site_dispatch_issued: None,
            graph: InvestigationGraph::new(),
        }
    }

    pub(crate) fn configure_usage_evidence_policy(&mut self, broad_usage_lookup: bool) {
        self.broad_usage_lookup = broad_usage_lookup;
    }

    pub(crate) fn evidence_ready(&self) -> bool {
        self.search_produced_results
            && self.useful_accepted_candidate_reads >= self.useful_candidate_reads_target
    }

    pub(crate) fn all_useful_accepted_reads_are_definition_only(&self) -> bool {
        self.useful_accepted_candidate_reads > 0
            && self.useful_accepted_candidate_paths.iter().all(|p| {
                self.definition_only_candidates
                    .iter()
                    .any(|d| normalize_evidence_path(d) == *p)
            })
    }

    pub(crate) fn has_non_definition_candidates(&self) -> bool {
        self.has_non_definition_candidates
    }

    pub(crate) fn search_produced_results(&self) -> bool {
        self.search_produced_results
    }

    pub(crate) fn files_read_count(&self) -> usize {
        self.files_read_count
    }

    pub(crate) fn candidate_reads_count(&self) -> usize {
        self.candidate_reads_count
    }

    pub(crate) fn useful_candidate_reads_count(&self) -> usize {
        self.useful_accepted_candidate_reads
    }

    #[cfg(test)]
    pub(crate) fn useful_candidate_reads_target_for_test(&self) -> usize {
        self.useful_candidate_reads_target
    }

    pub(crate) fn search_attempted(&self) -> bool {
        self.search_attempted
    }

    pub(crate) fn non_candidate_read_attempts(&self) -> usize {
        self.non_candidate_read_attempts
    }

    /// Increments the non-candidate read attempt counter and returns the new count.
    /// Called in run_tool_round before dispatch; persists across rounds within a turn.
    pub(crate) fn increment_non_candidate_read_attempts(&mut self) -> usize {
        self.non_candidate_read_attempts += 1;
        self.non_candidate_read_attempts
    }

    pub(crate) fn search_candidate_count(&self) -> usize {
        self.search_candidate_paths.len()
    }

    /// Returns the best candidate path for the given investigation mode.
    /// Routes to the mode-specific classifier first; falls back to the first search
    /// candidate if the mode has no dedicated set or that set is empty.
    pub(crate) fn best_candidate_for_mode(&self, mode: InvestigationMode) -> Option<&str> {
        let mode_specific = match mode {
            InvestigationMode::InitializationLookup => self.first_initialization_candidate(),
            InvestigationMode::ConfigLookup => self.first_config_candidate(),
            InvestigationMode::CreateLookup => self.first_create_candidate(),
            InvestigationMode::RegisterLookup => self.first_register_candidate(),
            InvestigationMode::CallSiteLookup => self.first_call_site_candidate(),
            InvestigationMode::LoadLookup => self.first_load_candidate(),
            InvestigationMode::SaveLookup => self.first_save_candidate(),
            InvestigationMode::DefinitionLookup => self.first_definition_candidate(),
            InvestigationMode::UsageLookup => {
                self.preferred_usage_candidate_with_filters(&HashSet::new(), false)
            }
            InvestigationMode::General => self.first_source_candidate(),
        };
        mode_specific.or_else(|| self.search_candidate_paths.first().map(String::as_str))
    }

    pub(crate) fn issue_direct_answer_correction(&mut self) -> bool {
        if self.direct_answer_correction_issued {
            return false;
        }
        self.direct_answer_correction_issued = true;
        true
    }

    pub(crate) fn issue_premature_synthesis_correction(&mut self) -> bool {
        if self.premature_synthesis_correction_issued {
            return false;
        }
        self.premature_synthesis_correction_issued = true;
        true
    }

    pub(crate) fn is_search_candidate_path(&self, path: &str) -> bool {
        let read_path = normalize_evidence_path(path);
        let relative_suffix = read_path.contains('/').then(|| format!("/{read_path}"));
        self.search_candidate_paths.iter().any(|candidate| {
            let candidate = normalize_evidence_path(candidate);
            candidate == read_path
                || relative_suffix
                    .as_ref()
                    .is_some_and(|suffix| candidate.ends_with(suffix))
        })
    }

    pub(crate) fn record_search_results(
        &mut self,
        output: &ToolOutput,
        query: Option<&str>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> bool {
        let ToolOutput::SearchResults(results) = output else {
            return false;
        };

        self.search_attempted = true;
        let was_empty = results.matches.is_empty();
        if !was_empty {
            self.search_produced_results = true;
            self.accepted_search_summaries.push(format!(
                "search: {} — {} matches",
                query.unwrap_or("?"),
                results.matches.len()
            ));
            self.search_candidate_paths.clear();
            self.definition_only_candidates.clear();
            self.non_definition_match_counts.clear();
            self.definition_site_candidates.clear();
            self.has_non_definition_candidates = false;
            self.import_only_candidates.clear();
            self.has_non_import_candidates = false;
            self.config_file_candidates.clear();
            self.has_non_config_candidates = false;
            self.initialization_candidates.clear();
            self.has_non_initialization_candidates = false;
            self.create_candidates.clear();
            self.has_non_create_candidates = false;
            self.register_candidates.clear();
            self.has_non_register_candidates = false;
            self.call_site_candidates.clear();
            self.has_non_call_site_candidates = false;
            self.load_candidates.clear();
            self.has_non_load_candidates = false;
            self.load_definition_only_candidates.clear();
            self.has_non_definition_load_candidates = false;
            self.save_candidates.clear();
            self.lockfile_candidates.clear();
            self.useful_accepted_candidate_reads = 0;
            self.useful_accepted_candidate_paths.clear();
            self.useful_candidate_reads_target = 1;

            for result in &results.matches {
                push_unique_path(&mut self.search_candidate_paths, &result.file);
            }

            // Classify each candidate file along structural axes.
            // definition-only: every matched line looks like a definition site (line-content).
            // import-only: every matched line looks like an import declaration (line-content).
            // config-file: the file's extension identifies it as a config file (path-based).
            // initialization: at least one matched line contains an exact initialization term.
            // create: at least one matched line contains an exact create term.
            // register: at least one matched line contains an exact register term.
            // load: at least one matched line contains an exact load term.
            // save: at least one matched line contains an exact save term.
            // lockfile: exact filename match against known lockfile basenames.
            let mut file_has_non_def: HashSet<String> = HashSet::new();
            let mut file_non_definition_counts: HashMap<String, usize> = HashMap::new();
            let mut file_has_exact_def: HashSet<String> = HashSet::new();
            let mut file_has_non_import: HashSet<String> = HashSet::new();
            let mut file_has_initialization: HashSet<String> = HashSet::new();
            let mut file_has_create: HashSet<String> = HashSet::new();
            let mut file_has_register: HashSet<String> = HashSet::new();
            let mut file_has_call_site: HashSet<String> = HashSet::new();
            let mut file_has_load: HashSet<String> = HashSet::new();
            let mut file_has_non_definition_load: HashSet<String> = HashSet::new();
            let mut file_has_save: HashSet<String> = HashSet::new();
            for m in &results.matches {
                if match query {
                    Some(sym) => !looks_like_definition_of_symbol(&m.line, sym),
                    None => !looks_like_definition(&m.line),
                } {
                    file_has_non_def.insert(m.file.clone());
                    *file_non_definition_counts
                        .entry(m.file.clone())
                        .or_insert(0) += 1;
                }
                if let Some(sym) = query {
                    if looks_like_definition_of_symbol(&m.line, sym) {
                        file_has_exact_def.insert(m.file.clone());
                    }
                }
                if !looks_like_import(&m.line) {
                    file_has_non_import.insert(m.file.clone());
                }
                if contains_initialization_term(&m.line) {
                    file_has_initialization.insert(m.file.clone());
                }
                if contains_create_term(&m.line) {
                    file_has_create.insert(m.file.clone());
                }
                if contains_register_term(&m.line) {
                    file_has_register.insert(m.file.clone());
                }
                let is_call_site_line = match query {
                    Some(sym) => looks_like_call_expression_of_symbol(&m.line, sym),
                    None => looks_like_call_expression(&m.line),
                };
                if is_call_site_line {
                    file_has_call_site.insert(m.file.clone());
                }
                if contains_load_term(&m.line) {
                    file_has_load.insert(m.file.clone());
                    let is_def = match query {
                        Some(sym) => looks_like_definition_of_symbol(&m.line, sym),
                        None => looks_like_definition(&m.line),
                    };
                    if !is_def {
                        file_has_non_definition_load.insert(m.file.clone());
                    }
                }
                if contains_save_term(&m.line) {
                    file_has_save.insert(m.file.clone());
                }
            }
            for path in &self.search_candidate_paths {
                if file_has_non_def.contains(path) {
                    self.has_non_definition_candidates = true;
                    if let Some(count) = file_non_definition_counts.get(path) {
                        self.non_definition_match_counts
                            .insert(path.clone(), *count);
                    }
                } else {
                    self.definition_only_candidates.insert(path.clone());
                }
                if file_has_exact_def.contains(path) {
                    self.definition_site_candidates.insert(path.clone());
                }
                if file_has_non_import.contains(path) {
                    self.has_non_import_candidates = true;
                } else {
                    self.import_only_candidates.insert(path.clone());
                }
                if is_config_file(path) {
                    self.config_file_candidates.insert(path.clone());
                } else {
                    self.has_non_config_candidates = true;
                }
                if file_has_initialization.contains(path) {
                    self.initialization_candidates.insert(path.clone());
                } else {
                    self.has_non_initialization_candidates = true;
                }
                if file_has_create.contains(path) {
                    self.create_candidates.insert(path.clone());
                } else {
                    self.has_non_create_candidates = true;
                }
                if file_has_register.contains(path) {
                    self.register_candidates.insert(path.clone());
                } else {
                    self.has_non_register_candidates = true;
                }
                if file_has_call_site.contains(path) {
                    self.call_site_candidates.insert(path.clone());
                } else {
                    self.has_non_call_site_candidates = true;
                }
                if file_has_load.contains(path) {
                    self.load_candidates.insert(path.clone());
                    if file_has_non_definition_load.contains(path) {
                        self.has_non_definition_load_candidates = true;
                    } else {
                        self.load_definition_only_candidates.insert(path.clone());
                    }
                } else {
                    self.has_non_load_candidates = true;
                }
                if file_has_save.contains(path) {
                    self.save_candidates.insert(path.clone());
                }
                if is_lockfile_path(path) {
                    self.lockfile_candidates.insert(path.clone());
                }
            }

            self.useful_candidate_reads_target = {
                let mut score: usize = 0;

                // broad usage lookup with multiple substantive candidates — known multi-site symbol.
                // Compound gate: broad alone does not raise target; needs at least two
                // substantive (non-definition-only, non-import-only, non-lockfile) candidates.
                if self.broad_usage_lookup && self.substantive_usage_candidate_count() >= 2 {
                    score += 1;
                }

                // many candidate files — symbol spans many files across the project
                if self.search_candidate_paths.len() >= 6 {
                    score += 1;
                }

                // high total match count — widely referenced symbol
                if results.total_matches >= 10 {
                    score += 1;
                }

                // graph already has edges from prior reads this session — cross-file context exists
                if self.graph.has_edges() {
                    score += 1;
                }

                // map score to target: 0→1, 1→2, 2→3, 3→4, 4+→5, never below 1 never above 5
                (score + 1).clamp(1, 5)
            };
        }
        trace_runtime_decision(
            on_event,
            "search_candidates_classified",
            &[
                ("shown_matches", results.matches.len().to_string()),
                ("total_matches", results.total_matches.to_string()),
                ("truncated", results.truncated.to_string()),
                (
                    "candidate_files",
                    self.search_candidate_paths.len().to_string(),
                ),
                (
                    "definition_only",
                    self.definition_only_candidates.len().to_string(),
                ),
                (
                    "has_non_definition",
                    self.has_non_definition_candidates.to_string(),
                ),
                ("import_only", self.import_only_candidates.len().to_string()),
                ("has_non_import", self.has_non_import_candidates.to_string()),
                (
                    "config_files",
                    self.config_file_candidates.len().to_string(),
                ),
                ("has_non_config", self.has_non_config_candidates.to_string()),
                (
                    "initialization_files",
                    self.initialization_candidates.len().to_string(),
                ),
                (
                    "has_non_initialization",
                    self.has_non_initialization_candidates.to_string(),
                ),
                ("create_files", self.create_candidates.len().to_string()),
                ("has_non_create", self.has_non_create_candidates.to_string()),
                ("register_files", self.register_candidates.len().to_string()),
                (
                    "has_non_register",
                    self.has_non_register_candidates.to_string(),
                ),
                (
                    "call_site_files",
                    self.call_site_candidates.len().to_string(),
                ),
                (
                    "has_non_call_site",
                    self.has_non_call_site_candidates.to_string(),
                ),
                ("load_files", self.load_candidates.len().to_string()),
                ("has_non_load", self.has_non_load_candidates.to_string()),
                (
                    "load_definition_only",
                    self.load_definition_only_candidates.len().to_string(),
                ),
                (
                    "has_non_definition_load",
                    self.has_non_definition_load_candidates.to_string(),
                ),
                ("save_files", self.save_candidates.len().to_string()),
                ("lockfiles", self.lockfile_candidates.len().to_string()),
                (
                    "useful_target",
                    self.useful_candidate_reads_target.to_string(),
                ),
                ("broad_usage_lookup", self.broad_usage_lookup.to_string()),
            ],
        );
        was_empty
    }

    pub(crate) fn record_read_result(
        &mut self,
        output: &ToolOutput,
        mode: InvestigationMode,
        classification: ReadClassification,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<(String, RecoveryKind)> {
        let ToolOutput::FileContents(file) = output else {
            return None;
        };

        self.files_read_count += 1;
        let read_path = normalize_evidence_path(&file.path);

        if classification == ReadClassification::Direct {
            self.direct_reads_count += 1;
            self.direct_read_paths.insert(read_path.clone());
        }

        let is_search_candidate = self
            .search_candidate_paths
            .iter()
            .any(|candidate| normalize_evidence_path(candidate) == read_path);

        if is_search_candidate {
            self.candidate_reads_count += 1;
            let is_def_only = self
                .definition_only_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_import_only = self
                .import_only_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_config_candidate = self
                .config_file_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_initialization_candidate = self
                .initialization_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_create_candidate = self
                .create_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_register_candidate = self
                .register_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_call_site_candidate = self
                .call_site_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_load_candidate = self
                .load_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_load_def_only = self
                .load_definition_only_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_save_candidate = self
                .save_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);
            let is_lockfile_candidate = self
                .lockfile_candidates
                .iter()
                .any(|c| normalize_evidence_path(c) == read_path);

            // Bypass: definition-site dispatch. If the runtime explicitly dispatched this
            // path after usage candidates were exhausted, accept it unconditionally.
            // Gate 1 must not reject a file the runtime was directed to read.
            if self.definition_site_dispatch_issued.as_deref() == Some(read_path.as_str()) {
                // Undo the candidate_reads_count increment above: definition-site reads are
                // supplemental runtime dispatches and must not consume a candidate slot.
                self.candidate_reads_count -= 1;
                self.useful_accepted_candidate_reads += 1;
                self.useful_accepted_candidate_paths.insert(read_path.clone());
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "true".into()),
                        ("reason", "definition_site_dispatch_bypass".into()),
                        ("candidate_reads", self.candidate_reads_count.to_string()),
                        (
                            "useful_candidate_reads",
                            self.useful_accepted_candidate_reads.to_string(),
                        ),
                    ],
                );
                return None;
            }
            // Gate 1 (UsageLookup): definition-only reads are structurally insufficient
            // when usage candidates exist. Fire once; subsequent reads fall through ungated.
            if matches!(mode, InvestigationMode::UsageLookup)
                && is_def_only
                && self.has_non_definition_candidates
            {
                if !self.premature_synthesis_correction_issued {
                    let suggested_path = self.first_non_definition_candidate().map(str::to_string);
                    if suggested_path.is_some() {
                        self.premature_synthesis_correction_issued = true;
                    }
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "usage_definition_only_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::DefinitionOnly));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        (
                            "reason",
                            "usage_definition_only_recovery_already_issued".into(),
                        ),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 2 (ConfigLookup): non-config reads are structurally insufficient when
            // config-file candidates exist. Fire once; fallback accepts if no config candidates.
            else if matches!(mode, InvestigationMode::ConfigLookup)
                && !is_config_candidate
                && !self.config_file_candidates.is_empty()
            {
                if !self.config_correction_issued {
                    self.config_correction_issued = true;
                    let suggested_path = self.first_config_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "config_non_config_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::ConfigFile));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "config_non_config_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 3 (InitializationLookup): non-initialization reads are structurally
            // insufficient when initialization candidates exist. Fire once; fallback
            // accepts if no initialization candidates exist.
            else if matches!(mode, InvestigationMode::InitializationLookup)
                && !is_initialization_candidate
                && !self.initialization_candidates.is_empty()
            {
                if !self.initialization_correction_issued {
                    self.initialization_correction_issued = true;
                    let suggested_path = self.first_initialization_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            (
                                "reason",
                                "initialization_non_initialization_candidate".into(),
                            ),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::Initialization));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "initialization_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 4 (CreateLookup): non-create reads are structurally insufficient when
            // create candidates exist. Fire once; fallback accepts if no create candidates.
            else if matches!(mode, InvestigationMode::CreateLookup)
                && !is_create_candidate
                && !self.create_candidates.is_empty()
            {
                if !self.create_correction_issued {
                    self.create_correction_issued = true;
                    let suggested_path = self.first_create_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "create_non_create_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::Create));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "create_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 5 (RegisterLookup): non-register reads are structurally insufficient when
            // register candidates exist. Fire once; fallback accepts if no register candidates.
            else if matches!(mode, InvestigationMode::RegisterLookup)
                && !is_register_candidate
                && !self.register_candidates.is_empty()
            {
                if !self.register_correction_issued {
                    self.register_correction_issued = true;
                    let suggested_path = self.first_register_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "register_non_register_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::Register));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "register_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 5.5 (CallSiteLookup): non-call-site reads are structurally insufficient when
            // call-site candidates exist. Fire once; fallback accepts if no call-site candidates.
            else if matches!(mode, InvestigationMode::CallSiteLookup)
                && !is_call_site_candidate
                && !self.call_site_candidates.is_empty()
            {
                if !self.call_site_correction_issued {
                    self.call_site_correction_issued = true;
                    let suggested_path = self.first_call_site_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "call_site_non_call_site_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::CallSite));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "call_site_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 6a (LoadLookup | General): load candidates whose load-term lines are all
            // definition sites are structurally insufficient when call-site load candidates exist.
            // Fire once; fall through if no call-site load candidates exist.
            else if matches!(mode, InvestigationMode::LoadLookup | InvestigationMode::General)
                && is_load_candidate
                && is_load_def_only
                && self.has_non_definition_load_candidates
            {
                if !self.load_definition_only_correction_issued {
                    let suggested_path =
                        self.first_non_definition_load_candidate().map(str::to_string);
                    if suggested_path.is_some() {
                        self.load_definition_only_correction_issued = true;
                    }
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "load_definition_only_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::LoadDefinitionOnly));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "load_definition_only_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 6 (LoadLookup): non-load reads are structurally insufficient when
            // load candidates exist. Fire once; fallback accepts if no load candidates.
            else if matches!(mode, InvestigationMode::LoadLookup)
                && !is_load_candidate
                && !self.load_candidates.is_empty()
            {
                if !self.load_correction_issued {
                    self.load_correction_issued = true;
                    let suggested_path = self.first_load_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "load_non_load_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::Load));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "load_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 7 (SaveLookup): non-save reads are structurally insufficient when
            // save candidates exist. Fire once; fallback accepts if no save candidates.
            else if matches!(mode, InvestigationMode::SaveLookup)
                && !is_save_candidate
                && !self.save_candidates.is_empty()
            {
                if !self.save_correction_issued {
                    self.save_correction_issued = true;
                    let suggested_path = self.first_save_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "save_non_save_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::Save));
                }
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "save_recovery_already_issued".into()),
                    ],
                );
                // Correction already issued: fall through without accepting.
            }
            // Gate 8 (DefinitionLookup): all non-definition-site reads are rejected.
            // A file passes when it is in definition_only_candidates (all lines are defs)
            // OR definition_site_candidates (at least one line is an exact definition).
            // When a definition candidate exists, RuntimeDispatch redirects to it.
            // When no definition candidate was classified, first_definition_candidate()
            // returns None and the gate returns None — the read does not satisfy evidence.
            else if matches!(mode, InvestigationMode::DefinitionLookup)
                && !is_def_only
                && !self
                    .definition_site_candidates
                    .iter()
                    .any(|c| normalize_evidence_path(c) == read_path)
            {
                let suggested_path = self.first_definition_candidate().map(str::to_string);
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "false".into()),
                        ("reason", "definition_lookup_non_definition_site".into()),
                        (
                            "recovery_path",
                            suggested_path.clone().unwrap_or_else(|| "none".into()),
                        ),
                    ],
                );
                return suggested_path.map(|p| (p, RecoveryKind::NonDefinitionSite));
            } else {
                if is_lockfile_candidate {
                    let suggested_path = self.first_source_candidate().map(str::to_string);
                    if suggested_path.is_some() {
                        if !self.lockfile_correction_issued {
                            self.lockfile_correction_issued = true;
                            trace_runtime_decision(
                                on_event,
                                "read_evidence",
                                &[
                                    ("path", read_path.clone()),
                                    ("accepted", "false".into()),
                                    ("reason", "lockfile_candidate".into()),
                                    (
                                        "recovery_path",
                                        suggested_path.clone().unwrap_or_else(|| "none".into()),
                                    ),
                                ],
                            );
                            return suggested_path.map(|p| (p, RecoveryKind::Lockfile));
                        }
                        trace_runtime_decision(
                            on_event,
                            "read_evidence",
                            &[
                                ("path", read_path.clone()),
                                ("accepted", "false".into()),
                                ("reason", "lockfile_recovery_already_issued".into()),
                            ],
                        );
                        return None;
                    }
                }
                // Candidate would normally be accepted. Check import-only before committing.
                // Import-only candidates are structurally insufficient when substantive
                // (non-import) candidates exist in the current result set.
                if is_import_only
                    && self.has_non_import_candidates
                    && !self.import_correction_issued
                {
                    self.import_correction_issued = true;
                    let suggested_path = self.first_non_import_candidate().map(str::to_string);
                    trace_runtime_decision(
                        on_event,
                        "read_evidence",
                        &[
                            ("path", read_path.clone()),
                            ("accepted", "false".into()),
                            ("reason", "import_only_candidate".into()),
                            (
                                "recovery_path",
                                suggested_path.clone().unwrap_or_else(|| "none".into()),
                            ),
                        ],
                    );
                    return suggested_path.map(|p| (p, RecoveryKind::ImportOnly));
                }
                self.useful_accepted_candidate_reads += 1;
                self.useful_accepted_candidate_paths
                    .insert(read_path.clone());
                trace_runtime_decision(
                    on_event,
                    "read_evidence",
                    &[
                        ("path", read_path.clone()),
                        ("accepted", "true".into()),
                        (
                            "reason",
                            self.acceptance_reason(mode, is_def_only, is_import_only),
                        ),
                        ("candidate_reads", self.candidate_reads_count.to_string()),
                        (
                            "useful_candidate_reads",
                            self.useful_accepted_candidate_reads.to_string(),
                        ),
                    ],
                );
            }
        } else {
            trace_runtime_decision(
                on_event,
                "read_evidence",
                &[
                    ("path", read_path),
                    ("accepted", "false".into()),
                    (
                        "reason",
                        if classification == ReadClassification::Direct {
                            "direct_read".into()
                        } else {
                            "not_search_candidate".into()
                        },
                    ),
                ],
            );
        }
        None
    }

    fn acceptance_reason(
        &self,
        mode: InvestigationMode,
        is_def_only: bool,
        is_import_only: bool,
    ) -> String {
        if matches!(mode, InvestigationMode::ConfigLookup) && self.config_file_candidates.is_empty()
        {
            "config_fallback_no_config_candidates".into()
        } else if matches!(mode, InvestigationMode::InitializationLookup)
            && self.initialization_candidates.is_empty()
        {
            "initialization_fallback_no_initialization_candidates".into()
        } else if matches!(mode, InvestigationMode::CreateLookup)
            && self.create_candidates.is_empty()
        {
            "create_fallback_no_create_candidates".into()
        } else if matches!(mode, InvestigationMode::RegisterLookup)
            && self.register_candidates.is_empty()
        {
            "register_fallback_no_register_candidates".into()
        } else if matches!(mode, InvestigationMode::CallSiteLookup)
            && self.call_site_candidates.is_empty()
        {
            "call_site_fallback_no_call_site_candidates".into()
        } else if matches!(mode, InvestigationMode::LoadLookup) && self.load_candidates.is_empty() {
            "load_fallback_no_load_candidates".into()
        } else if matches!(mode, InvestigationMode::SaveLookup) && self.save_candidates.is_empty() {
            "save_fallback_no_save_candidates".into()
        } else if matches!(mode, InvestigationMode::UsageLookup)
            && is_def_only
            && !self.has_non_definition_candidates
        {
            "usage_fallback_no_usage_candidates".into()
        } else if is_import_only && !self.has_non_import_candidates {
            "import_fallback_all_import_candidates".into()
        } else {
            "search_candidate".into()
        }
    }

    fn first_non_definition_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| !self.definition_only_candidates.contains(*path))
            .map(String::as_str)
    }

    pub(crate) fn preferred_usage_candidate(&self) -> Option<String> {
        if let Some(path) = self.preferred_usage_candidate_with_filters(&HashSet::new(), false) {
            return Some(path.to_string());
        }
        self.graph.promoted_candidates().into_iter().next()
    }

    pub(crate) fn next_usage_evidence_candidate(&self) -> Option<&str> {
        if self.useful_accepted_candidate_reads == 0
            || self.useful_accepted_candidate_reads >= self.useful_candidate_reads_target
        {
            return None;
        }

        self.preferred_usage_candidate_with_filters(&self.useful_accepted_candidate_paths, true)
    }

    fn preferred_usage_candidate_with_filters(
        &self,
        excluded: &HashSet<String>,
        substantive_only: bool,
    ) -> Option<&str> {
        let mut best_index = None;
        let mut best_key = None;

        for (index, path) in self.search_candidate_paths.iter().enumerate() {
            if excluded.contains(&normalize_evidence_path(path)) {
                continue;
            }
            if substantive_only && !self.is_substantive_usage_candidate(path) {
                continue;
            }
            let key = self.usage_candidate_quality_key(path, index);
            if best_key.as_ref().is_none_or(|current| key < *current) {
                best_key = Some(key);
                best_index = Some(index);
            }
        }

        best_index.map(|index| self.search_candidate_paths[index].as_str())
    }

    fn substantive_usage_candidate_count(&self) -> usize {
        self.search_candidate_paths
            .iter()
            .filter(|path| self.is_substantive_usage_candidate(path))
            .count()
    }

    fn is_substantive_usage_candidate(&self, path: &str) -> bool {
        !self.definition_only_candidates.contains(path)
            && !self.import_only_candidates.contains(path)
            && !self.lockfile_candidates.contains(path)
    }

    pub(crate) fn first_definition_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| {
                self.definition_only_candidates.contains(*path)
                    || self.definition_site_candidates.contains(*path)
            })
            .map(String::as_str)
    }

    /// Returns the first candidate that contains an exact definition of the queried symbol,
    /// regardless of whether it is also in definition_only_candidates. Used by the
    /// UsageLookup supplemental dispatch after all usage candidates are exhausted.
    pub(crate) fn first_definition_site_candidate(&self) -> Option<String> {
        if let Some(path) = self
            .search_candidate_paths
            .iter()
            .find(|path| self.definition_site_candidates.contains(*path))
        {
            return Some(path.clone());
        }
        self.graph.promoted_candidates().into_iter().next()
    }

    fn first_non_import_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| !self.import_only_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_config_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.config_file_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_initialization_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.initialization_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_create_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.create_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_register_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.register_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_call_site_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.call_site_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_load_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.load_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_non_definition_load_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| {
                self.load_candidates.contains(*path)
                    && !self.load_definition_only_candidates.contains(*path)
            })
            .map(String::as_str)
    }

    fn first_save_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| self.save_candidates.contains(*path))
            .map(String::as_str)
    }

    fn first_source_candidate(&self) -> Option<&str> {
        self.search_candidate_paths
            .iter()
            .find(|path| {
                !self.lockfile_candidates.contains(*path) && is_source_candidate_path(path)
            })
            .map(String::as_str)
    }

    fn usage_candidate_quality_key(&self, path: &str, index: usize) -> (u8, u8, u8, usize, usize) {
        let is_definition_only = self.definition_only_candidates.contains(path);
        let is_import_only = self.import_only_candidates.contains(path);
        let is_normal_source = is_source_candidate_path(path)
            && !self.config_file_candidates.contains(path)
            && !self.initialization_candidates.contains(path)
            && !self.lockfile_candidates.contains(path);
        let non_definition_match_count = self
            .non_definition_match_counts
            .get(path)
            .copied()
            .unwrap_or(0);

        (
            u8::from(is_definition_only),
            u8::from(is_import_only),
            u8::from(!is_normal_source),
            usize::MAX - non_definition_match_count,
            index,
        )
    }

    /// Returns a mode-specific candidate preference hint after a search, or None when:
    /// - no search candidates are recorded yet, or
    /// - all candidates are already of the preferred type (no misdirection possible), or
    /// - the mode has no preferred candidate class (General, UsageLookup, DefinitionLookup).
    ///
    /// DefinitionLookup is intentionally excluded: the definition_site_file preamble in
    /// tool_codec already handles that case directly in the rendered search output.
    pub(crate) fn candidate_preference_hint(&self, mode: InvestigationMode) -> Option<String> {
        if self.search_candidate_paths.is_empty() {
            return None;
        }
        match mode {
            InvestigationMode::InitializationLookup
                if !self.initialization_candidates.is_empty()
                    && self.has_non_initialization_candidates =>
            {
                let path = self.first_initialization_candidate()?;
                Some(format!(
                    "[initialization match found in {path} — read this file first]"
                ))
            }
            InvestigationMode::ConfigLookup
                if !self.config_file_candidates.is_empty() && self.has_non_config_candidates =>
            {
                let path = self.first_config_candidate()?;
                Some(format!(
                    "[config file found in {path} — read this file first]"
                ))
            }
            InvestigationMode::CreateLookup
                if !self.create_candidates.is_empty() && self.has_non_create_candidates =>
            {
                let path = self.first_create_candidate()?;
                Some(format!(
                    "[create match found in {path} — read this file first]"
                ))
            }
            InvestigationMode::RegisterLookup
                if !self.register_candidates.is_empty() && self.has_non_register_candidates =>
            {
                let path = self.first_register_candidate()?;
                Some(format!(
                    "[register match found in {path} — read this file first]"
                ))
            }
            InvestigationMode::CallSiteLookup
                if !self.call_site_candidates.is_empty() && self.has_non_call_site_candidates =>
            {
                let path = self.first_call_site_candidate()?;
                Some(format!(
                    "[call site found in {path} — read this file first]"
                ))
            }
            InvestigationMode::LoadLookup
                if !self.load_candidates.is_empty() && self.has_non_load_candidates =>
            {
                let path = self.first_load_candidate()?;
                Some(format!(
                    "[load match found in {path} — read this file first]"
                ))
            }
            InvestigationMode::SaveLookup if !self.save_candidates.is_empty() => {
                let has_non_save = self
                    .search_candidate_paths
                    .iter()
                    .any(|p| !self.save_candidates.contains(p));
                if has_non_save {
                    let path = self.first_save_candidate()?;
                    Some(format!(
                        "[save match found in {path} — read this file first]"
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub(crate) fn set_definition_site_dispatched(&mut self, path: &str) {
        self.definition_site_dispatch_issued = Some(normalize_evidence_path(path));
    }

    pub fn evidence_summary(&self) -> Vec<String> {
        let mut items = Vec::new();
        for path in &self.useful_accepted_candidate_paths {
            items.push(format!("read: {}", path));
        }
        for s in &self.accepted_search_summaries {
            items.push(s.clone());
        }
        items
    }
}
