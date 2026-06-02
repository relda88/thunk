use crate::llm::backend::BackendTimingStage;
use crate::tools::PendingAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Processing,
    LoadingModel,
    CreatingContext,
    Tokenizing,
    Prefilling,
    Generating {
        mode: Option<String>,
    },
    Responding,
    ExecutingTools {
        tool: String,
        detail: Option<String>,
    },
    AwaitingApproval {
        tool: String,
    },
}

impl Activity {
    pub fn label(self) -> String {
        match self {
            Self::Idle => "ready".to_string(),
            Self::Processing => "processing...".to_string(),
            Self::LoadingModel => "loading model...".to_string(),
            Self::CreatingContext => "creating context...".to_string(),
            Self::Tokenizing => "tokenizing...".to_string(),
            Self::Prefilling => "prefilling...".to_string(),
            Self::Generating { mode: Some(m) } => format!("{}...", m),
            Self::Generating { mode: None } => "generating...".to_string(),
            Self::Responding => "responding".to_string(),
            Self::ExecutingTools {
                tool,
                detail: Some(d),
            } => format!("{}: {}", tool, d),
            Self::ExecutingTools { tool, detail: None } => format!("{}...", tool),
            Self::AwaitingApproval { tool } => format!("approval: {}", tool),
        }
    }
}

/// Describes why the tool loop terminated and how the final answer was reached.
#[derive(Debug, Clone)]
pub enum AnswerSource {
    /// Model produced a final answer without using any tools.
    Direct,
    /// A final answer was produced after one or more tool rounds.
    ToolAssisted { rounds: usize },
    /// Runtime produced a deterministic terminal answer without model synthesis.
    RuntimeTerminal {
        reason: RuntimeTerminalReason,
        rounds: usize,
    },
    /// Loop was cut off at the tool round limit before a final answer.
    ToolLimitReached,
}

/// Runtime-owned terminal outcomes. These are policy decisions, not model output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTerminalReason {
    RejectedMutation,
    ReadFileFailed,
    /// A mutation tool call was rejected at resolver level (e.g. path escapes project root).
    /// Distinct from RejectedMutation, which is a user-initiated cancellation of an approved action.
    MutationFailed,
    RepeatedDisallowedTool,
    RepeatedSearchBudgetViolation,
    RepeatedFabricatedToolResult,
    RepeatedMalformedToolSyntax,
    RepeatedGarbledEditRepair,
    RepeatedToolAfterEvidenceReady,
    RepeatedWeakSearchQuery,
    /// Model attempted further tool use after the turn's artifact was already acquired.
    RepeatedToolAfterAnswerPhase,
    /// Search was attempted but all results were empty and no file was read.
    /// The runtime emits the answer directly rather than letting the model speculate.
    InsufficientEvidence,
}

/// External inputs the runtime accepts from the app/TUI layer.
#[derive(Debug, Clone)]
pub enum RuntimeRequest {
    Submit {
        text: String,
    },
    /// Clears conversation history and resets to a fresh session.
    Reset,
    /// Confirms a pending tool action, allowing execute_approved() to run.
    Approve,
    /// Cancels a pending tool action without executing it.
    Reject,
    /// Read-only query: returns the last assistant message as an InfoMessage event.
    /// Does not mutate conversation state or trigger session save.
    QueryLast,
    /// Read-only query: returns current anchor state as an InfoMessage event.
    /// Does not mutate any state or trigger session save.
    QueryAnchors,
    /// Read-only query: returns bounded recent conversation history as an InfoMessage event.
    /// Does not mutate any state or trigger session save.
    QueryHistory,
    /// Command-triggered read_file invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    ReadFile {
        path: String,
    },
    /// Command-triggered search_code invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    SearchCode {
        query: String,
    },
    /// Reverts the most recent approved mutation by restoring the file's prior contents.
    /// No-op with a user message if the undo stack is empty.
    Undo,
    /// Lists all known providers and indicates which is currently active.
    ProvidersList,
    /// Switches the active backend provider by name.
    ProvidersUse {
        name: String,
    },
    /// Command-triggered git_branch invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    GitBranch,
    /// Command-triggered git_status invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    GitStatus,
    /// Command-triggered git_diff invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    GitDiff,
    /// Command-triggered git_log invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    GitLog,
    /// Command-triggered list_dir invocation. Goes through CommandTool allowlist.
    /// Does not mutate conversation or trigger session save.
    ListDir {
        path: String,
    },
    /// Read-only LSP health query. Returns LSP status as a SystemMessage event.
    /// Does not mutate conversation state or trigger session save.
    LspStatus,
    /// Runs the symbol extractor and writes results to the index store.
    /// `large` disables the default file-count guard for large projects.
    /// Does not mutate conversation state or trigger session save.
    IndexBuild {
        large: bool,
    },
    /// Read-only index status query. Returns symbol count, import count, and last
    /// build time as a SystemMessage event.
    IndexStatus,
    /// Read-only context stats query. Returns token estimate, message count, tool
    /// result count, oldest tool result age, and context window percentage as a
    /// SystemMessage event.  Does not mutate conversation state or trigger session save.
    ContextStats,
    /// Prunes stale small tool results from the live conversation in-place using the
    /// same heuristic as `pruned_snapshot()`.  Emits a SystemMessage with the count
    /// of pruned results.  Does not trigger session save.
    Compact,
    /// Session-scoped prompt physics toggle. `Some(true)` enables, `Some(false)` disables,
    /// `None` queries current status. Does not mutate conversation or trigger session save.
    PromptPhysicsToggle {
        enabled: Option<bool>,
    },
    /// Session-scoped verify command setter. `Some("off")` disables, `Some(cmd)` sets
    /// the verify command, `None` queries current status. Does not mutate conversation
    /// or trigger session save.
    VerifyMutationToggle {
        command: Option<String>,
    },
}

/// Events emitted by the runtime for UI rendering, logging, and lifecycle handling.
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    ActivityChanged(Activity),
    AssistantMessageStarted,
    AssistantMessageChunk(String),
    AssistantMessageFinished,
    ToolCallStarted {
        name: String,
    },
    /// Fired when a tool completes. `summary` is a compact one-line render of the
    /// result for TUI display. `None` means the tool failed.
    ToolCallFinished {
        name: String,
        summary: Option<String>,
    },
    /// Fired when a mutating tool requires user approval before execution.
    /// The turn is paused until RuntimeRequest::Approve or Reject is received.
    ApprovalRequired {
        pending: PendingAction,
        evidence: Vec<String>,
    },
    AnswerReady(AnswerSource),
    Failed {
        message: String,
    },
    /// Informational output from a read-only runtime query (e.g. /last).
    /// Rendered by the TUI as a system message; never added to conversation state.
    InfoMessage(String),
    /// Advisory timing event routed from the backend. Consumed by the logging layer only;
    /// must not be forwarded to the TUI or drive any control flow.
    BackendTiming {
        stage: BackendTimingStage,
        elapsed_ms: u64,
    },
    /// Advisory token count event routed from the backend. Consumed by the logging layer only;
    /// must not be forwarded to the TUI or drive any control flow.
    BackendTokenCounts {
        prompt: u32,
        completion: u32,
    },
    /// Advisory runtime decision trace. Consumed by the application logging layer only;
    /// must not be forwarded to the TUI or drive any control flow.
    RuntimeTrace(String),
    /// The fully formatted prompt string assembled just before backend generation.
    /// Captured by the TUI for prompt inspection; must not affect control flow.
    PromptAssembled(String),
    /// A runtime-generated message for the user that is not assistant output.
    /// Displayed as a system message in the TUI; never added to conversation state.
    SystemMessage(String),
    /// Fired after a successful read_file completion. Carries the full file content
    /// for the TUI expand view. Advisory only — must not affect control flow.
    FileReadFinished {
        path: String,
        line_count: usize,
        content: String,
    },
    /// Fired after a direct read turn completes and the fallback answer has been
    /// streamed. The TUI uses this to record the assistant message index for Ctrl+O.
    DirectReadCompleted,
    /// Fired at the end of each turn with approximate context window usage for the TUI indicator.
    /// `prompt_tokens` is the actual token count if available, otherwise a char-based estimate
    /// (prompt chars / 4). Only fired when context_window_tokens is known from the backend.
    ContextUsage {
        prompt_tokens: u64,
        context_window_tokens: u32,
    },
}
