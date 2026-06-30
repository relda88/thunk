use std::collections::HashSet;

use crate::tools::ToolInput;

use super::prompt_analysis::normalized_prompt_tokens;

/// Runtime-owned per-turn tool surface.
///
/// A surface defines which read-only tool family is available for the current
/// turn. This is policy enforced by the runtime before dispatch; tools and
/// tool_codec must not own or interpret surface rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolSurface {
    RetrievalFirst,
    GitReadOnly,
    /// Synthesis-only surface: no tools offered.
    /// Used for answer-phase generations after evidence is accepted or a read completes,
    /// to prevent the model from attempting tool calls and triggering a correction round.
    AnswerOnly,
    /// Read tools plus approval-required tools (edit_file, write_file, shell) visible in the per-turn hint.
    /// Selected when the prompt requests a mutation so the model knows those tools are
    /// available this turn. Enforcement for mutation calls remains the same as RetrievalFirst:
    /// they bypass surface checks via the approval path.
    MutationEnabled,
}

/// Canonical registry entry for a tool surface.
///
/// Keeping the surface name and allowed tools together prevents hint rendering
/// and enforcement from drifting apart.
struct ToolSurfaceDefinition {
    surface: ToolSurface,
    name: &'static str,
    tools: &'static [SurfaceTool],
}

/// Runtime policy view of model-callable tools.
///
/// Mutation tools are intentionally excluded from surfaces because approval and
/// mutation permission are governed by a separate lifecycle path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceTool {
    SearchCode,
    ReadFile,
    ListDir,
    GitStatus,
    GitDiff,
    GitLog,
    GitBranch,
    GitDiffStaged,
    LspDefinition,
    ShellRead,
}

const RETRIEVAL_FIRST_TOOLS: &[SurfaceTool] = &[
    SurfaceTool::SearchCode,
    SurfaceTool::ReadFile,
    SurfaceTool::ListDir,
    SurfaceTool::LspDefinition,
    SurfaceTool::ShellRead,
];
const GIT_READ_ONLY_TOOLS: &[SurfaceTool] = &[
    SurfaceTool::GitStatus,
    SurfaceTool::GitDiff,
    SurfaceTool::GitLog,
    SurfaceTool::GitBranch,
    SurfaceTool::GitDiffStaged,
];
const ANSWER_ONLY_TOOLS: &[SurfaceTool] = &[];
// MutationEnabled has the same read tools as RetrievalFirst. Approval-required tools
// (edit_file, write_file, shell) are not SurfaceTool variants — they bypass surface
// enforcement and are exposed to the model only via the mutation_tool_names() hint extension.
const MUTATION_ENABLED_TOOLS: &[SurfaceTool] = &[
    SurfaceTool::SearchCode,
    SurfaceTool::ReadFile,
    SurfaceTool::ListDir,
    SurfaceTool::ShellRead,
];
const TOOL_SURFACE_DEFINITIONS: &[ToolSurfaceDefinition] = &[
    ToolSurfaceDefinition {
        surface: ToolSurface::RetrievalFirst,
        name: "RetrievalFirst",
        tools: RETRIEVAL_FIRST_TOOLS,
    },
    ToolSurfaceDefinition {
        surface: ToolSurface::GitReadOnly,
        name: "GitReadOnly",
        tools: GIT_READ_ONLY_TOOLS,
    },
    ToolSurfaceDefinition {
        surface: ToolSurface::AnswerOnly,
        name: "AnswerOnly",
        tools: ANSWER_ONLY_TOOLS,
    },
    ToolSurfaceDefinition {
        surface: ToolSurface::MutationEnabled,
        name: "MutationEnabled",
        tools: MUTATION_ENABLED_TOOLS,
    },
];

impl SurfaceTool {
    pub(crate) fn from_input(input: &ToolInput) -> Option<Self> {
        match input {
            ToolInput::SearchCode { .. } => Some(Self::SearchCode),
            ToolInput::ReadFile { .. } => Some(Self::ReadFile),
            ToolInput::ListDir { .. } => Some(Self::ListDir),
            ToolInput::GitStatus => Some(Self::GitStatus),
            ToolInput::GitDiff => Some(Self::GitDiff),
            ToolInput::GitLog => Some(Self::GitLog),
            ToolInput::GitBranch => Some(Self::GitBranch),
            ToolInput::GitDiffStaged => Some(Self::GitDiffStaged),
            ToolInput::ShellRead { .. } => Some(Self::ShellRead),
            ToolInput::EditFile { .. }
            | ToolInput::WriteFile { .. }
            | ToolInput::Shell { .. }
            | ToolInput::GitBranchCreate { .. }
            | ToolInput::GitBranchSwitch { .. }
            | ToolInput::GitCommit { .. }
            | ToolInput::WebFetch { .. }
            | ToolInput::DynamicTool { .. } => None,
            ToolInput::LspDefinition { .. } => Some(Self::LspDefinition),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::SearchCode => "search_code",
            Self::ReadFile => "read_file",
            Self::ListDir => "list_dir",
            Self::GitStatus => "git_status",
            Self::GitDiff => "git_diff",
            Self::GitLog => "git_log",
            Self::GitBranch => "git_branch",
            Self::GitDiffStaged => "git_diff_staged",
            Self::LspDefinition => "lsp_definition",
            Self::ShellRead => "shell_read",
        }
    }
}

impl ToolSurface {
    fn definition(self) -> &'static ToolSurfaceDefinition {
        TOOL_SURFACE_DEFINITIONS
            .iter()
            .find(|definition| definition.surface == self)
            .expect("tool surface definition must exist")
    }

    pub(crate) fn as_str(self) -> &'static str {
        self.definition().name
    }

    pub(crate) fn tools(self) -> &'static [SurfaceTool] {
        self.definition().tools
    }

    pub(crate) fn allowed_tool_names(self) -> impl Iterator<Item = &'static str> {
        self.tools().iter().copied().map(SurfaceTool::name)
    }

    /// Returns the mutation tool names that should be appended to the per-turn hint
    /// when this surface is active. Empty for all surfaces except MutationEnabled.
    pub(crate) fn mutation_tool_names(self) -> &'static [&'static str] {
        match self {
            Self::MutationEnabled => &["edit_file", "write_file", "shell"],
            _ => &[],
        }
    }

    pub(crate) fn includes_project_snapshot_hint(self) -> bool {
        matches!(self, Self::RetrievalFirst | Self::MutationEnabled)
    }
}

pub(crate) fn select_tool_surface(
    prompt: &str,
    investigation_required: bool,
    mutation_allowed: bool,
    has_direct_read: bool,
) -> ToolSurface {
    if is_explicit_git_tooling_prompt(prompt) {
        ToolSurface::GitReadOnly
    } else if mutation_allowed {
        ToolSurface::MutationEnabled
    } else if investigation_required
        || has_direct_read
        || prompt_requests_directory_navigation(prompt)
    {
        ToolSurface::RetrievalFirst
    } else {
        ToolSurface::AnswerOnly
    }
}

/// Returns true only for explicit Git read-only requests.
///
/// The accepted phrases are narrow by design; code-investigation prompts that
/// merely mention "git" should remain RetrievalFirst. Bare "git" (single token)
/// is mapped to GitReadOnly so the user can invoke git tools by typing just "git".
/// Git subcommand tokens are matched anywhere in the token stream (not just prefix)
/// so natural questions like "what git branch am I on?" are correctly classified.
fn is_explicit_git_tooling_prompt(prompt: &str) -> bool {
    let tokens = normalized_prompt_tokens(prompt);
    // Bare "git" — single-token input routes to GitReadOnly.
    if tokens == ["git"] {
        return true;
    }
    // Prefix-style matches (legacy, retained for specificity).
    starts_with_token_phrase(&tokens, &["show", "git", "status"])
        || starts_with_token_phrase(&tokens, &["show", "git", "diff"])
        || starts_with_token_phrase(&tokens, &["show", "git", "log"])
        || starts_with_token_phrase(&tokens, &["git", "status"])
        || starts_with_token_phrase(&tokens, &["git", "diff"])
        || starts_with_token_phrase(&tokens, &["git", "log"])
        || starts_with_token_phrase(&tokens, &["show", "working", "tree"])
        || starts_with_token_phrase(&tokens, &["show", "recent", "commits"])
        || starts_with_token_phrase(&tokens, &["show", "latest", "commits"])
        || starts_with_token_phrase(&tokens, &["show", "recent", "git", "status"])
        || starts_with_token_phrase(&tokens, &["show", "recent", "git", "diff"])
        || starts_with_token_phrase(&tokens, &["show", "recent", "git", "log"])
        || starts_with_token_phrase(&tokens, &["show", "latest", "git", "status"])
        || starts_with_token_phrase(&tokens, &["show", "latest", "git", "diff"])
        || starts_with_token_phrase(&tokens, &["show", "latest", "git", "log"])
        || starts_with_token_phrase(&tokens, &["git", "branch"])
        || starts_with_token_phrase(&tokens, &["show", "git", "branch"])
        || starts_with_token_phrase(&tokens, &["what", "branch"])
        || starts_with_token_phrase(&tokens, &["which", "branch"])
        || starts_with_token_phrase(&tokens, &["current", "branch"])
        || starts_with_token_phrase(&tokens, &["show", "current", "branch"])
        // Contains-style matches: question-word + "git" + subcommand anywhere in the stream.
        // Narrow to question-word-prefixed patterns only to avoid false-positives on code
        // investigation prompts like "where is git status rendered".
        || contains_token_phrase(&tokens, &["what", "git", "status"])
        || contains_token_phrase(&tokens, &["what", "git", "diff"])
        || contains_token_phrase(&tokens, &["what", "git", "log"])
        || contains_token_phrase(&tokens, &["what", "git", "branch"])
        || contains_token_phrase(&tokens, &["what", "git", "commit"])
        || contains_token_phrase(&tokens, &["what", "git", "push"])
        || contains_token_phrase(&tokens, &["what", "git", "pull"])
        || contains_token_phrase(&tokens, &["which", "git", "status"])
        || contains_token_phrase(&tokens, &["which", "git", "diff"])
        || contains_token_phrase(&tokens, &["which", "git", "log"])
        || contains_token_phrase(&tokens, &["which", "git", "branch"])
        || contains_token_phrase(&tokens, &["which", "git", "commit"])
        || contains_token_phrase(&tokens, &["which", "git", "push"])
        || contains_token_phrase(&tokens, &["which", "git", "pull"])
}

fn prompt_requests_directory_navigation(prompt: &str) -> bool {
    let tokens = normalized_prompt_tokens(prompt);
    const NAV_VERBS: &[&str] = &["list", "show", "display", "tree", "explore"];
    const STRUCTURAL_KEYWORDS: &[&str] = &[
        "files",
        "file",
        "directory",
        "dir",
        "dirs",
        "structure",
        "contents",
        "folders",
        "folder",
    ];
    let has_nav_verb = tokens.iter().any(|t| NAV_VERBS.contains(&t.as_str()));
    if !has_nav_verb {
        return false;
    }
    let has_structural_keyword = tokens
        .iter()
        .any(|t| STRUCTURAL_KEYWORDS.contains(&t.as_str()));
    let has_path_token = prompt.split_whitespace().any(|t| t.contains('/'));
    has_structural_keyword || has_path_token
}

fn starts_with_token_phrase(tokens: &[String], phrase: &[&str]) -> bool {
    tokens.len() >= phrase.len()
        && tokens
            .iter()
            .take(phrase.len())
            .map(String::as_str)
            .eq(phrase.iter().copied())
}

/// Returns true if `phrase` appears as a contiguous subsequence anywhere in `tokens`.
fn contains_token_phrase(tokens: &[String], phrase: &[&str]) -> bool {
    if phrase.is_empty() || tokens.len() < phrase.len() {
        return false;
    }
    tokens
        .windows(phrase.len())
        .any(|w| w.iter().map(String::as_str).eq(phrase.iter().copied()))
}

/// Enforces whether a tool call is available on the active surface.
///
/// Mutation calls return true here because they are checked by the separate
/// approval/mutation policy, not by read-only surface enforcement.
/// `dynamic_allowed` is the runtime-held set of dynamic tool names permitted on any
/// surface; it is empty until MCP tools are wired in (Slice 45.3).
pub(crate) fn tool_allowed_for_surface(
    input: &ToolInput,
    surface: ToolSurface,
    dynamic_allowed: &HashSet<String>,
) -> bool {
    if let Some(tool) = SurfaceTool::from_input(input) {
        // Direct membership check: is this read-only tool in the surface's canonical set?
        // Using direct lookup avoids ambiguity when multiple surfaces share the same tools
        // (e.g., MutationEnabled and RetrievalFirst both carry search/read/list).
        surface.tools().contains(&tool)
    } else if dynamic_allowed.contains(input.tool_name()) {
        // Dynamic read-only tool explicitly whitelisted for this surface.
        true
    } else {
        // Mutation permission remains separate from tool-surface policy.
        // Unknown tools and approval-required tools (edit_file, write_file, shell) pass through.
        true
    }
}

/// Identifies Git read-only tool calls for Git acquisition/finalization logic.
pub(crate) fn is_git_read_only_tool_input(input: &ToolInput) -> bool {
    matches!(
        SurfaceTool::from_input(input).and_then(tool_surface_for_tool),
        Some(ToolSurface::GitReadOnly)
    )
}

fn tool_surface_for_tool(tool: SurfaceTool) -> Option<ToolSurface> {
    TOOL_SURFACE_DEFINITIONS
        .iter()
        .find(|definition| definition.tools.contains(&tool))
        .map(|definition| definition.surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_git_maps_to_git_readonly() {
        assert!(is_explicit_git_tooling_prompt("git"));
    }

    #[test]
    fn natural_git_branch_question_maps_to_git_readonly() {
        assert!(is_explicit_git_tooling_prompt("what git branch am I on?"));
        assert!(is_explicit_git_tooling_prompt("which git branch is active"));
        assert!(is_explicit_git_tooling_prompt(
            "what git status does this repo have"
        ));
    }

    #[test]
    fn investigation_prompts_with_git_terms_stay_retrieval_first() {
        // Code investigation prompts that happen to mention git terms must not
        // be mis-classified as GitReadOnly by the contains matching.
        assert!(!is_explicit_git_tooling_prompt(
            "where is git status rendered"
        ));
        assert!(!is_explicit_git_tooling_prompt(
            "find the git integration code"
        ));
    }
}
