use std::collections::HashSet;
use std::sync::Arc;

use crate::core::config::{Config, InvestigationDepth, RetrievalConfig};
use crate::llm::backend::ModelBackend;
use crate::runtime::index::EmbeddingProvider;
use crate::storage::index::store::SymbolRecord;
use crate::storage::index::SymbolStore;
use crate::storage::retrieval::RetrievalLogStore;
use crate::storage::tasks::{EditSequenceStore, TaskStore};
use crate::tools::{
    PendingAction, PendingApprovalStage, PendingTransaction, ToolInput, ToolOutput, ToolRegistry,
    ToolRunResult,
};

use super::super::lsp::LspManager;

use super::super::conversation::Conversation;
use super::super::investigation::anchors::{
    has_same_scope_reference, is_last_read_file_anchor_prompt, is_last_search_anchor_prompt,
    AnchorState,
};
use super::super::investigation::investigation::{
    detect_investigation_mode, InvestigationMode, ReadClassification,
};
use super::super::paths::normalize_evidence_path;
use super::super::project::CargoContext;
use super::super::project::ProjectRoot;
use super::super::project::ProjectStructureSnapshot;
use super::super::project::ProjectStructureSnapshotCache;
use super::super::protocol::abilities::AbilityContent;
use super::super::protocol::prompt;
use super::super::protocol::prompt_physics::PromptPhysicsConfig;
use super::super::protocol::skills::SkillContent;
use super::super::protocol::tool_codec;
use super::super::resolve;
use super::super::types::{
    Activity, AnswerSource, RuntimeEvent, RuntimeRequest, RuntimeTerminalReason,
};
use super::context_policy::ContextPolicy;
use super::generation::{emit_visible_assistant_message, run_generate_turn};
use super::tool_round::{run_tool_round, ToolRoundOutcome};

#[path = "anchor_resolution.rs"]
mod anchor_resolution;

#[path = "command_handlers.rs"]
mod command_handlers;

#[path = "retrieval_log_writer.rs"]
mod retrieval_log_writer;

#[path = "answer_admission.rs"]
mod answer_admission;

#[path = "answer_guard.rs"]
mod answer_guard;

#[path = "plan_handlers.rs"]
mod plan_handlers;

#[path = "memory_handlers.rs"]
mod memory_handlers;

#[path = "embed_handlers.rs"]
mod embed_handlers;

fn capture_session_head(root: &std::path::Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Maximum tool rounds per turn. Prevents runaway loops when the model keeps
/// producing tool calls without reaching a final answer.
const MAX_TOOL_ROUNDS: usize = 10;

/// Maximum automatic corrections per turn. One correction is enough — if the
/// model fabricates twice in a row the prompt fix is insufficient and we surface
/// the failure rather than looping silently.
const MAX_CORRECTIONS: usize = 1;

use super::super::protocol::response_text::*;
use super::super::trace::trace_runtime_decision;
use super::context_cap::{cap_tool_result_blocks, estimate_generation_prompt_chars};
use super::engine_guards::usage_lookup_is_broad;
use super::telemetry::{
    infer_post_tool_round_cause, short_tool_name, tool_input_activity,
    trace_insufficient_evidence_terminal, GenerationRoundCause, GenerationRoundLabel,
};

use super::super::investigation::tool_surface::{select_tool_surface, ToolSurface};

use super::turn_state::{
    AnswerPhaseKind, DeepeningState, PendingRuntimeCall, TurnContext, TurnSignal, TurnState,
};

/// Returns true if the prompt contains a token that looks like a code identifier.
/// Only two structural patterns are checked — no NLP, no heuristics.
use super::super::investigation::prompt_analysis::{
    classify_retrieval_intent, extract_investigation_path_scope, is_permitted_shell_command,
    prompt_requires_investigation, requested_shell_command, requested_simple_edit,
    user_requested_execution, user_requested_mutation, DirectReadMode, RetrievalIntent,
};

pub struct Runtime {
    #[allow(dead_code)]
    project_root: ProjectRoot,
    conversation: Conversation,
    backend: Box<dyn ModelBackend>,
    registry: ToolRegistry,
    system_prompt: String,
    pub(crate) anchors: AnchorState,
    context_policy: ContextPolicy,
    project_snapshot_cache: ProjectStructureSnapshotCache,
    /// Holds a mutating tool action that is waiting for user approval.
    /// Set when a tool round suspends; cleared by Approve or Reject.
    /// At most one pending action exists at any time.
    /// The stage tracks whether the pre-edit LSP safety check has run.
    pending_action: Option<PendingApprovalStage>,
    config: Config,
    /// Queued runtime-owned tool call to execute at the start of the next run_turns invocation.
    /// Set by handle_approve when a post-mutation follow-up (e.g. test run) is configured.
    pending_runtime_call: Option<PendingRuntimeCall>,
    /// Per-session undo stack. Each entry is (absolute_path, before_contents).
    /// Empty string for before_contents means the file did not exist before write_file created it.
    /// Capped at 5 entries — oldest dropped when exceeded.
    undo_stack: Vec<(String, String)>,
    /// Persistent LSP server session. Starts lazily on first query when lsp.enabled = true.
    /// Shut down in Drop via graceful shutdown → kill.
    lsp: LspManager,
    /// MCP server manager. None when no mcp.json config exists or all servers fail to start.
    /// Advisory — session proceeds normally when absent. Shut down in Drop via kill+wait.
    mcp_manager: Option<crate::runtime::mcp::MCPManager>,
    /// MCP tools discovered at session start via `tools/list`, with namespaced names.
    /// Session-scoped — re-discovered fresh each session, never persisted to SQLite.
    /// Empty when no MCP servers are configured or none expose tools.
    discovered_tools: Vec<crate::runtime::mcp::McpTool>,
    /// Personal memory manager. None when ~/.thunk/memory.db is unavailable or memory disabled.
    /// Advisory — session proceeds normally when absent. MemoryStore drops cleanly with no
    /// explicit shutdown needed.
    pub(crate) memory_manager: Option<crate::runtime::memory::MemoryManager>,
    /// Symbol index store. `None` when no db_path was supplied (e.g. in tests).
    pub(super) symbol_store: Option<SymbolStore>,
    /// Embedding provider for vector search. `None` when unconfigured.
    pub(super) embedding_provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>,
    pub(super) retrieval_config: RetrievalConfig,
    /// Plan/task store. `None` when no db_path was supplied (e.g. in tests).
    pub(crate) task_store: Option<TaskStore>,
    /// Edit sequence store. `None` when no db_path was supplied (e.g. in tests).
    pub(crate) edit_store: Option<EditSequenceStore>,
    /// ID of the sequence currently being executed. Set by handle_sequence_approve,
    /// cleared on Completed or Failed. None = no active sequence.
    pub(crate) active_sequence_id: Option<String>,
    /// Set to true after the first on-demand index build attempt this session.
    /// Ensures the trigger fires at most once per session.
    pub(super) index_triggered: bool,
    /// Set to true after the 75% context warning fires. Cleared on reset so the
    /// warning re-arms for the next session.
    pub(super) context_75_warned: bool,
    prompt_physics: PromptPhysicsConfig,
    /// Session-scoped verify command: run after every approved edit_file/write_file
    /// mutation. None = disabled. Initialized from config.project.verify_command;
    /// can be changed at runtime via /verify <command>|off without restarting.
    verify_command: Option<String>,
    /// When true, verification runs in a background thread after the turn completes
    /// (display-only, no correction loop). When false, verification runs synchronously
    /// with the correction loop. Defaults to true.
    deferred_verify: bool,
    /// Tracks how many correction attempts have been made for the current mutation.
    /// Reset to 0 on cargo check success, exhaustion, or when corrections are disabled.
    correction_attempts: u32,
    /// Maximum allowed correction attempts per mutation. From config.project.max_correction_attempts.
    max_correction_attempts: u32,
    /// SHA-1 of HEAD at the time this Runtime was constructed.
    /// Used as the baseline for /diff last. Never mutated after new().
    session_start_ref: Option<String>,
    /// Path to the .thunk/ project directory. Used by AbilityLoader and SkillLoader
    /// to locate ability/skill files at toggle time.
    thunk_dir: std::path::PathBuf,
    /// Active reasoning ability for this session. Loaded at toggle time; None = no ability set.
    active_ability: Option<AbilityContent>,
    /// Active response style skill for this session. Loaded at toggle time; None = no skill set.
    active_skill: Option<SkillContent>,
    /// Session-scoped constrained output. When true, tool-call turns on
    /// supported backends use grammar-constrained or JSON-mode generation.
    /// Initialized from backend config; overridable via /constrain on|off.
    constrained_output: bool,
    /// Whether /fetch is enabled. Read from config.web_fetch.enabled at startup.
    web_fetch_enabled: bool,
    /// Session identity. Used by plan handlers to key TaskStore queries.
    session_id: String,
    /// Parsed plan awaiting user approval. Set by handle_plan_create, consumed by
    /// handle_plan_approve / handle_plan_abandon. Never persisted; cleared on reset.
    pending_plan: Option<command_handlers::PendingPlanDraft>,
    /// Proposed memory fact awaiting user approval. Set by propose_memory, cleared by approve/reject.
    pending_memory: Option<crate::storage::memory::MemoryFact>,
    /// True when pending_memory represents a deletion proposal rather than a write.
    pending_memory_is_delete: bool,
    /// Queue of memory proposals from /reflect. Drained one-at-a-time after each approve/reject.
    pending_memory_queue: Vec<crate::storage::memory::MemoryFact>,
    /// In-progress chunked embed state between IndexEmbedChunk dispatches.
    /// Set by handle_index_embed, consumed by handle_index_embed_chunk, cleared on reset.
    pub(super) pending_embed: Option<embed_handlers::PendingEmbedState>,
    /// Session-scoped investigation depth. Initialized from config.investigation.depth;
    /// overridable at runtime via /depth <shallow|normal|deep>.
    investigation_depth: InvestigationDepth,
    /// Maximum deepening hops in deep mode. From config.investigation.hop_limit.
    investigation_hop_limit: usize,
    /// Hard cap on total reads per turn in deep mode. From config.investigation.max_total_reads.
    investigation_max_reads: usize,
    /// Retrieval quality log. `None` when no db_path was supplied (e.g. in tests).
    retrieval_log_store: Option<RetrievalLogStore>,
    /// True after the low-hit-rate warning has been emitted this session.
    /// Ensures the warning fires at most once per session.
    retrieval_warn_emitted: bool,
}

impl Runtime {
    pub fn new(
        config: &Config,
        project_root: ProjectRoot,
        backend: Box<dyn ModelBackend>,
        registry: ToolRegistry,
        thunk_md: Option<String>,
        thunk_dir: std::path::PathBuf,
        session_id: String,
    ) -> Self {
        let specs = registry.specs();
        let prompt_physics = PromptPhysicsConfig {
            enabled: config.prompt_physics.enabled,
            compress_abilities: config.prompt_physics.compress_abilities,
            thunk_md,
            active_ability: None,
            active_skill: None,
        };
        let cargo_context = CargoContext::load(project_root.path());
        let context_policy = ContextPolicy::from_capabilities(backend.capabilities());
        let lsp = LspManager::new(&config.lsp, project_root.path());
        // MCP must initialize before build_system_prompt so discovered tools appear in the
        // model's prompt from session start.
        let mut mcp_manager = {
            use crate::runtime::mcp::{MCPManager, McpConfig};
            let home_mcp_config = std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".thunk").join("mcp.json"));
            let home = home_mcp_config.as_deref().filter(|p| p.exists());
            let project_path = thunk_dir.join("mcp.json");
            let project = if project_path.exists() {
                Some(project_path.as_path())
            } else {
                None
            };
            let config = McpConfig::load(home, project);
            if config.servers.is_empty() {
                None
            } else {
                let mut mgr = MCPManager::new(config);
                mgr.start_all();
                Some(mgr)
            }
        };
        // Discover tools eagerly. Static tool names are never overwritten — a namespaced
        // MCP tool colliding with a static name is skipped with a warning.
        let static_names: std::collections::HashSet<&str> = specs.iter().map(|s| s.name).collect();
        let discovered_tools: Vec<crate::runtime::mcp::McpTool> = mcp_manager
            .as_mut()
            .map(|m| m.discover_all())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| {
                if static_names.contains(t.name.as_str()) {
                    eprintln!("MCP tool name collision: {}, skipping", t.name);
                    false
                } else {
                    true
                }
            })
            .collect();
        let system_prompt = prompt::build_system_prompt(
            &config.app.name,
            project_root.path(),
            cargo_context.as_ref(),
            &specs,
            false,
            &prompt_physics,
            &discovered_tools,
            &[], // anchor facts injected later by with_memory_manager
        );
        let session_start_ref = capture_session_head(project_root.path());
        Self {
            project_root,
            conversation: Conversation::new(system_prompt.clone()),
            backend,
            registry,
            system_prompt,
            anchors: AnchorState::default(),
            context_policy,
            project_snapshot_cache: ProjectStructureSnapshotCache::default(),
            pending_action: None,
            config: config.clone(),
            pending_runtime_call: None,
            undo_stack: Vec::new(),
            lsp,
            mcp_manager,
            discovered_tools,
            memory_manager: None,
            symbol_store: None,
            embedding_provider: None,
            retrieval_config: config.retrieval.clone(),
            task_store: None,
            edit_store: None,
            active_sequence_id: None,
            index_triggered: false,
            context_75_warned: false,
            prompt_physics,
            verify_command: config.project.verify_command.clone(),
            deferred_verify: true,
            correction_attempts: 0,
            max_correction_attempts: config.project.max_correction_attempts,
            session_start_ref,
            thunk_dir,
            active_ability: None,
            active_skill: None,
            constrained_output: config.llama_cpp.use_grammar || config.ollama.constrained_output,
            web_fetch_enabled: config.web_fetch.enabled,
            session_id,
            pending_plan: None,
            pending_memory: None,
            pending_memory_is_delete: false,
            pending_memory_queue: Vec::new(),
            pending_embed: None,
            investigation_depth: config.investigation.depth,
            investigation_hop_limit: config.investigation.hop_limit,
            investigation_max_reads: config.investigation.max_total_reads,
            retrieval_log_store: None,
            retrieval_warn_emitted: false,
        }
    }

    /// Attaches a `SymbolStore` backed by `db_path`. Returns `self` for chaining.
    /// Silently proceeds without a store if the path cannot be opened.
    pub fn with_symbol_store(mut self, db_path: &std::path::Path) -> Self {
        self.symbol_store = SymbolStore::open(db_path).ok();
        self
    }

    /// Attaches a `TaskStore` backed by `db_path`. Returns `self` for chaining.
    /// Silently proceeds without a store if the path cannot be opened.
    pub fn with_task_store(mut self, db_path: &std::path::Path) -> Self {
        self.task_store = TaskStore::open(db_path).ok();
        self
    }

    /// Attaches an `EditSequenceStore`. Returns `self` for chaining.
    pub fn with_edit_store(mut self, store: EditSequenceStore) -> Self {
        self.edit_store = Some(store);
        self
    }

    /// Attaches a `RetrievalLogStore` backed by `db_path`. Returns `self` for chaining.
    /// Silently proceeds without a store if the path cannot be opened.
    pub fn with_retrieval_log_store(mut self, db_path: &std::path::Path) -> Self {
        self.retrieval_log_store = RetrievalLogStore::open(db_path).ok();
        self
    }

    /// Attaches an embedding provider for vector search. Returns `self` for chaining.
    pub fn with_embedding_provider(
        mut self,
        provider: Arc<dyn EmbeddingProvider + Send + Sync>,
    ) -> Self {
        self.embedding_provider = Some(provider);
        self
    }

    /// Attaches a personal memory manager. Returns `self` for chaining.
    /// Advisory — call only when the home DB is confirmed available.
    pub fn with_memory_manager(mut self, manager: crate::runtime::memory::MemoryManager) -> Self {
        let root_str = self.project_root.path().to_string_lossy().into_owned();
        let anchor_facts = manager.anchor_facts(Some(&root_str), self.config.memory.anchor_limit);
        if !anchor_facts.is_empty() {
            self.system_prompt.push_str("## What I know about you\n");
            for fact in &anchor_facts {
                self.system_prompt.push_str(&format!("- {}\n", fact.text));
            }
            self.system_prompt.push('\n');
            self.conversation.reset(self.system_prompt.clone());
        }
        self.memory_manager = Some(manager);
        self
    }

    /// Returns the memory scope to use when proposing a new fact for persistence.
    /// None (global) when no .git ancestor was found; Some(project_root) otherwise.
    /// Read operations always use Some(project_root) — the SQL filter returns global
    /// facts in both cases. This helper governs writes only.
    fn memory_write_scope(&self) -> Option<String> {
        if self.project_root.path().join(".git").exists() {
            Some(self.project_root.path().to_string_lossy().into_owned())
        } else {
            None
        }
    }

    #[cfg(test)]
    pub fn with_prompt_physics_enabled(mut self) -> Self {
        self.prompt_physics.enabled = true;
        self
    }

    #[cfg(test)]
    pub fn with_constrained_output(mut self) -> Self {
        self.constrained_output = true;
        self
    }

    #[cfg(test)]
    pub fn with_verify_command(mut self, cmd: Option<String>) -> Self {
        self.verify_command = cmd;
        self
    }

    /// Returns the verify command and project root path for background execution.
    /// None when no verify command is configured.
    pub fn verify_context(&self) -> Option<(String, std::path::PathBuf)> {
        self.verify_command
            .as_ref()
            .map(|cmd| (cmd.clone(), self.project_root.as_path_buf()))
    }

    #[cfg(test)]
    pub fn with_deferred_verify(mut self, deferred: bool) -> Self {
        self.deferred_verify = deferred;
        self
    }

    #[cfg(test)]
    pub fn with_max_correction_attempts(mut self, n: u32) -> Self {
        self.max_correction_attempts = n;
        self
    }

    /// Returns a snapshot of all current conversation messages for persistence.
    pub fn messages_snapshot(&self) -> Vec<crate::llm::backend::Message> {
        self.conversation.snapshot()
    }

    /// Appends historical messages into the conversation after the system prompt.
    /// Called once at startup when restoring a prior session. Not for use mid-turn.
    pub fn load_history(&mut self, messages: Vec<crate::llm::backend::Message>) {
        self.conversation.extend_history(messages);
    }

    /// Restores anchor state persisted from a prior session.
    /// Called once at startup after session restore, parallel to load_history.
    /// Uses the existing anchor update mechanism so invariants are preserved.
    pub fn restore_anchors(
        &mut self,
        last_read_file: Option<String>,
        last_search_query: Option<String>,
        last_search_scope: Option<String>,
    ) {
        if let Some(path) = last_read_file {
            let output =
                crate::tools::ToolOutput::FileContents(crate::tools::types::FileContentsOutput {
                    path,
                    contents: String::new(),
                    total_lines: 0,
                    truncated: false,
                });
            self.anchors.record_successful_read(&output);
        }
        if let Some(query) = last_search_query {
            let output =
                crate::tools::ToolOutput::SearchResults(crate::tools::types::SearchResultsOutput {
                    query: query.clone(),
                    matches: vec![],
                    total_matches: 0,
                    truncated: false,
                });
            self.anchors
                .record_successful_search(&output, query, last_search_scope);
        }
    }

    /// Returns a snapshot of the current anchor state for persistence.
    pub fn anchors_snapshot(&self) -> (Option<String>, Option<String>, Option<String>) {
        let last_read_file = self.anchors.last_read_file().map(str::to_string);
        let (last_search_query, last_search_scope) = match self.anchors.last_search() {
            Some((q, s)) => (Some(q), s),
            None => (None, None),
        };
        (last_read_file, last_search_query, last_search_scope)
    }

    /// Rebuilds the symbol index for a single file. Best-effort — silently no-ops
    /// if the store is absent or empty. Intended for watcher-triggered incremental updates.
    pub fn rebuild_file(&mut self, path: &std::path::Path) {
        self.rebuild_index_for_file(path, &mut |_| {});
    }

    /// Handles a RuntimeRequest by updating the conversation, invoking the backend,
    /// and firing RuntimeEvents to drive the UI. Each request type has its own
    /// handler method for clarity.
    pub fn handle(&mut self, request: RuntimeRequest, on_event: &mut dyn FnMut(RuntimeEvent)) {
        match request {
            RuntimeRequest::Submit { text } => {
                self.handle_submit(text, on_event);
                self.maybe_trigger_index_build(on_event);
            }
            RuntimeRequest::Reset => self.handle_reset(on_event),
            RuntimeRequest::Approve => self.handle_approve(on_event),
            RuntimeRequest::Reject => self.handle_reject(on_event),
            RuntimeRequest::QueryLast => self.handle_query_last(on_event),
            RuntimeRequest::QueryAnchors => self.handle_query_anchors(on_event),
            RuntimeRequest::QueryHistory => self.handle_query_history(on_event),
            RuntimeRequest::ReadFile { path } => self.handle_read_file(path, on_event),
            RuntimeRequest::SearchCode { query } => {
                self.handle_search_code(query, on_event);
                self.maybe_trigger_index_build(on_event);
            }
            RuntimeRequest::Undo => self.handle_undo(on_event),
            RuntimeRequest::ProvidersList => self.handle_providers_list(on_event),
            RuntimeRequest::ProvidersUse { name } => self.handle_providers_use(name, on_event),
            RuntimeRequest::GitBranch => self.handle_git_branch(on_event),
            RuntimeRequest::GitStatus => self.handle_git_status(on_event),
            RuntimeRequest::GitDiff => self.handle_git_diff(on_event),
            RuntimeRequest::GitLog => self.handle_git_log(on_event),
            RuntimeRequest::BranchCreate {
                name,
                start_point: _,
            } => self.handle_branch_create(name, on_event),
            RuntimeRequest::BranchSwitch { name } => self.handle_branch_switch(name, on_event),
            RuntimeRequest::Commit { message } => self.handle_commit(message, on_event),
            RuntimeRequest::Diff { mode } => self.handle_diff(mode, on_event),
            RuntimeRequest::ListDir { path } => self.handle_list_dir(path, on_event),
            RuntimeRequest::LspStatus => self.handle_lsp_status(on_event),
            RuntimeRequest::McpList => self.handle_mcp_list(on_event),
            RuntimeRequest::McpRefresh => self.handle_mcp_refresh(on_event),
            RuntimeRequest::IndexBuild { large } => self.handle_index_build(large, on_event),
            RuntimeRequest::IndexStatus => self.handle_index_status(on_event),
            RuntimeRequest::IndexEmbed => self.handle_index_embed(on_event),
            RuntimeRequest::IndexEmbedChunk => self.handle_index_embed_chunk(on_event),
            RuntimeRequest::ContextStats => self.handle_context_stats(on_event),
            RuntimeRequest::Compact => self.handle_compact(on_event),
            RuntimeRequest::PromptPhysicsToggle { enabled } => {
                self.handle_prompt_physics_toggle(enabled, on_event)
            }
            RuntimeRequest::VerifyMutationToggle { command } => {
                self.handle_verify_mutation_toggle(command, on_event)
            }
            RuntimeRequest::TransactionStatus => self.handle_transaction_status(on_event),
            RuntimeRequest::AbilityToggle { name } => self.handle_ability_toggle(name, on_event),
            RuntimeRequest::SkillToggle { name } => self.handle_skill_toggle(name, on_event),
            RuntimeRequest::FetchUrl { url } => self.handle_fetch_url(url, on_event),
            RuntimeRequest::PlanCreate { goal } => self.handle_plan_create(goal, on_event),
            RuntimeRequest::PlanApprove => self.handle_plan_approve(on_event),
            RuntimeRequest::PlanAbandon => self.handle_plan_abandon(on_event),
            RuntimeRequest::PlanStatus => self.handle_plan_status(on_event),
            RuntimeRequest::TaskExecute { step } => self.handle_task_execute(step, on_event),
            RuntimeRequest::TaskComplete { step, summary } => {
                self.handle_task_complete(step, summary, on_event)
            }
            RuntimeRequest::TaskBlock { step, reason } => {
                self.handle_task_block(step, reason, on_event)
            }
            RuntimeRequest::TaskStatus => self.handle_task_status(on_event),
            RuntimeRequest::AgentRun { ability, target } => {
                self.handle_agent_run(ability, target, on_event)
            }
            RuntimeRequest::InvestigationDepthToggle { depth } => {
                self.handle_depth_toggle(depth, on_event)
            }
            RuntimeRequest::RetrievalLog { n } => self.handle_retrieval_log(n, on_event),
            RuntimeRequest::ConstrainedOutputToggle { enabled } => {
                self.handle_constrained_output_toggle(enabled, on_event)
            }
            RuntimeRequest::CompressToggle { enabled } => {
                self.handle_compress_toggle(enabled, on_event)
            }
            RuntimeRequest::Refactor { target } => self.handle_refactor(target, on_event),
            RuntimeRequest::SequenceApprove => self.handle_sequence_approve(on_event),
            RuntimeRequest::SequenceExecuteStep => self.handle_sequence_execute_step(on_event),
            RuntimeRequest::SequenceAbort => self.handle_sequence_abort(on_event),
            RuntimeRequest::SequenceStatus => self.handle_sequence_status(on_event),
            RuntimeRequest::MemoryApprove => self.handle_memory_approve(on_event),
            RuntimeRequest::MemoryReject => self.handle_memory_reject(on_event),
            RuntimeRequest::Remember { fact } => self.handle_remember(fact, on_event),
            RuntimeRequest::MemoryList => self.handle_memory_list(on_event),
            RuntimeRequest::MemoryForget { id } => self.handle_memory_forget(id, on_event),
            RuntimeRequest::Reflect => self.handle_reflect(on_event),
        }
    }

    /// Applies the Layer 1 context cap then commits the results to the conversation.
    /// Must be used for all tool-origin push_user calls so the cap is applied consistently.
    fn commit_tool_results(&mut self, results: String) {
        let capped = cap_tool_result_blocks(&results, self.context_policy.tool_result_max_lines);
        self.conversation.push_user(capped);
    }

    /// Iterative deepening phase: follow import-chain and definition edges from already-read
    /// files for up to `investigation_hop_limit` additional reads, bounded by
    /// `investigation_max_reads`. Only called when `investigation_depth == Deep` and the
    /// initial candidate reads are exhausted without satisfying evidence gates.
    ///
    /// Loops internally until: evidence gates satisfied, hop limit hit, read budget
    /// exhausted, or no more promoted candidates available. Returns `TurnSignal::Continue`
    /// when evidence becomes ready (answer_phase set), or calls `finish_with_runtime_answer`
    /// and returns `TurnSignal::Finish` when all options are exhausted.
    ///
    /// Note: call-site following is not available — the index has no callers table. Only
    /// import chains (InvestigationGraph import edges) and definition-site edges are used.
    pub(super) fn run_deepening_hop(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> TurnSignal {
        state.deepening.get_or_insert(DeepeningState {
            current_hop: 0,
            reads_this_deep_phase: 0,
        });

        loop {
            let (current_hop, reads_this_deep_phase) = {
                let d = state.deepening.as_ref().unwrap();
                (d.current_hop, d.reads_this_deep_phase)
            };

            // Stop: hop limit or total read budget exhausted
            if current_hop >= self.investigation_hop_limit
                || state.reads_this_turn.len() + reads_this_deep_phase
                    >= self.investigation_max_reads
            {
                trace_insufficient_evidence_terminal(
                    "deepening_exhausted",
                    state.tool_rounds,
                    &state.search_budget,
                    &state.investigation,
                    on_event,
                );
                self.finish_with_runtime_answer(
                    ungrounded_investigation_final_answer(),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return TurnSignal::Finish;
            }

            // Get next promoted candidate (import/definition edges from already-read files)
            let candidates = state.investigation.graph.promoted_candidates();
            let candidate = candidates.into_iter().find(|path| {
                let norm = normalize_evidence_path(path);
                !state.reads_this_turn.contains(&norm)
            });

            let Some(path) = candidate else {
                // No more reachable candidates at this depth — terminal
                trace_insufficient_evidence_terminal(
                    "deepening_no_more_candidates",
                    state.tool_rounds,
                    &state.search_budget,
                    &state.investigation,
                    on_event,
                );
                self.finish_with_runtime_answer(
                    ungrounded_investigation_final_answer(),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return TurnSignal::Finish;
            };

            // Project confinement: resolve enforces that path stays within project root
            let resolved = match resolve(
                &self.project_root,
                &ToolInput::ReadFile { path: path.clone() },
            ) {
                Ok(r) => r,
                Err(_) => {
                    // Path escapes root or is invalid — skip this candidate, advance hop
                    if let Some(d) = state.deepening.as_mut() {
                        d.current_hop += 1;
                    }
                    continue;
                }
            };

            // Register as a search candidate so evidence gates in record_read_result apply
            state.investigation.register_deepening_candidate(&path);

            // Dispatch read directly — bypasses run_tool_round (no per-turn cap check)
            let run_result = match self.registry.dispatch(resolved) {
                Ok(r) => r,
                Err(_) => {
                    if let Some(d) = state.deepening.as_mut() {
                        d.current_hop += 1;
                    }
                    continue;
                }
            };

            let output = match run_result {
                ToolRunResult::Immediate(o) => o,
                ToolRunResult::Approval(_) => {
                    // Read tools never require approval — defensive skip
                    if let Some(d) = state.deepening.as_mut() {
                        d.current_hop += 1;
                    }
                    continue;
                }
            };

            // Record graph edges from the file's import declarations
            if let ToolOutput::FileContents(ref fc) = output {
                state
                    .investigation
                    .graph
                    .record_read(&fc.path, &fc.contents);
                let norm = normalize_evidence_path(&fc.path);
                state.reads_this_turn.insert(norm);
            }

            // Run through evidence gates (record_read_result classifies the file)
            let _recovery = state.investigation.record_read_result(
                &output,
                ctx.investigation_mode,
                ReadClassification::Candidate,
                on_event,
            );

            // Commit result to conversation context
            let result_text = tool_codec::format_tool_result("read_file", &output);
            self.commit_tool_results(result_text);

            // Increment deepening counters and emit trace
            let (new_hop, new_reads) = {
                let d = state.deepening.as_mut().unwrap();
                d.reads_this_deep_phase += 1;
                d.current_hop += 1;
                (d.current_hop, d.reads_this_deep_phase)
            };

            let norm_path = match &output {
                ToolOutput::FileContents(fc) => normalize_evidence_path(&fc.path),
                _ => normalize_evidence_path(&path),
            };
            trace_runtime_decision(
                on_event,
                "deepening_hop_read",
                &[
                    ("path", norm_path),
                    ("hop", (new_hop - 1).to_string()),
                    ("reads_this_deep_phase", new_reads.to_string()),
                ],
            );

            // Evidence gates satisfied — transition to answer phase
            if state.investigation.evidence_ready() {
                state.answer_phase = Some(AnswerPhaseKind::InvestigationEvidenceReady);
                return TurnSignal::Continue;
            }
        }
    }

    fn get_or_build_project_snapshot(&mut self) -> std::io::Result<&ProjectStructureSnapshot> {
        self.project_snapshot_cache.get_or_build(&self.project_root)
    }

    fn maybe_render_project_snapshot_hint(&mut self, tool_surface: ToolSurface) -> Option<String> {
        if !tool_surface.includes_project_snapshot_hint() {
            return None;
        }

        let snapshot = self.get_or_build_project_snapshot().ok()?;
        Some(prompt::render_project_snapshot_hint(snapshot))
    }

    fn maybe_render_test_coverage_hint(&self, tool_surface: ToolSurface) -> Option<String> {
        if tool_surface != ToolSurface::MutationEnabled {
            return None;
        }
        let store = self.symbol_store.as_ref()?;
        let target_file = self.anchors.last_read_file()?;
        let project_root_str = self.project_root.path().to_string_lossy();
        let tests = store
            .test_importers_of(&project_root_str, target_file, 5)
            .ok()?;
        if tests.is_empty() {
            return None;
        }
        Some(format!(
            "[test coverage]\nTests referencing this file: {}\n[/test coverage]",
            tests.join(", ")
        ))
    }

    fn invalidate_project_snapshot(&mut self) {
        self.project_snapshot_cache.invalidate();
    }

    fn invalidate_project_snapshot_if_needed(&mut self, output: &ToolOutput) {
        if matches!(
            output,
            ToolOutput::WriteFile(_) | ToolOutput::EditFile(_) | ToolOutput::Shell(_)
        ) {
            self.invalidate_project_snapshot();
        }
    }

    fn handle_submit(&mut self, text: String, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::Failed {
                message:
                    "Cannot submit while a tool approval is pending. Use /approve or /reject first."
                        .to_string(),
            });
            return;
        }
        if self.pending_memory.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "finish the pending approval first — ^Y to confirm, ^N to discard".to_string(),
            ));
            return;
        }
        if self.pending_plan.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "finish the pending approval first — ^Y to confirm, ^N to discard".to_string(),
            ));
            return;
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            on_event(RuntimeEvent::Failed {
                message: "Cannot submit an empty prompt.".to_string(),
            });
            return;
        }

        if self.config.memory.enabled {
            if let Some(fact) =
                crate::runtime::investigation::prompt_analysis::user_requested_remember(trimmed)
            {
                let scope = self.memory_write_scope();
                self.propose_memory(
                    fact,
                    "user".to_string(),
                    scope,
                    crate::storage::memory::MemorySource::User,
                    on_event,
                );
                return;
            }
        }

        let is_last_read_file_anchor = is_last_read_file_anchor_prompt(trimmed);
        let is_last_search_anchor = is_last_search_anchor_prompt(trimmed);
        self.conversation.push_user(text);
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        if is_last_read_file_anchor {
            trace_runtime_decision(
                on_event,
                "anchor_prompt_matched",
                &[("kind", "last_read_file".into())],
            );
            if let Some(path) = self.anchors.last_read_file().map(str::to_string) {
                trace_runtime_decision(
                    on_event,
                    "anchor_resolved",
                    &[("kind", "last_read_file".into()), ("path", path.clone())],
                );
                self.run_last_read_file_anchor(path, on_event);
            } else {
                trace_runtime_decision(
                    on_event,
                    "anchor_missing",
                    &[("kind", "last_read_file".into())],
                );
                self.finish_with_runtime_answer(
                    NO_LAST_READ_FILE_AVAILABLE,
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::ReadFileFailed,
                        rounds: 0,
                    },
                    on_event,
                );
            }
            return;
        }
        if is_last_search_anchor {
            trace_runtime_decision(
                on_event,
                "anchor_prompt_matched",
                &[("kind", "last_search".into())],
            );
            if let Some((query, scope)) = self.anchors.last_search() {
                trace_runtime_decision(
                    on_event,
                    "anchor_resolved",
                    &[
                        ("kind", "last_search".into()),
                        ("query", query.clone()),
                        ("scope", scope.clone().unwrap_or_else(|| "none".into())),
                    ],
                );
                self.run_last_search_anchor(query, scope, on_event);
            } else {
                trace_runtime_decision(
                    on_event,
                    "anchor_missing",
                    &[("kind", "last_search".into())],
                );
                self.finish_with_runtime_answer(
                    NO_LAST_SEARCH_AVAILABLE,
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: 0,
                    },
                    on_event,
                );
            }
            return;
        }
        self.run_turns(0, on_event);
    }

    fn handle_approve(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let stage = match self.pending_action.take() {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::Failed {
                    message: "No pending action to approve.".to_string(),
                });
                return;
            }
        };

        match stage {
            PendingApprovalStage::AwaitingPreCheck(tx) => {
                if tx.is_single() {
                    let pending = tx.first().clone();
                    let is_file_mutation =
                        matches!(pending.tool_name.as_str(), "edit_file" | "write_file");
                    if is_file_mutation && self.lsp.is_enabled() {
                        if let Some(abs_path) = extract_absolute_path_from_payload(&pending.payload)
                        {
                            let path = std::path::Path::new(&abs_path);
                            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                            if path.exists()
                                && self.lsp.config().extensions.contains(&ext.to_string())
                            {
                                if let Ok(source) = std::fs::read_to_string(path) {
                                    if let Ok(diags) = self.lsp.query_diagnostics(path, &source) {
                                        let errors: Vec<_> = diags
                                            .iter()
                                            .filter(|d| d.severity == "error")
                                            .collect();
                                        if !errors.is_empty() {
                                            let evidence: Vec<String> = errors
                                                .iter()
                                                .take(4)
                                                .map(|d| {
                                                    format!("line {}: {}", d.line + 1, d.message)
                                                })
                                                .collect();
                                            self.pending_action =
                                                Some(PendingApprovalStage::PreCheckComplete(
                                                    PendingTransaction::single(pending.clone()),
                                                ));
                                            on_event(RuntimeEvent::ApprovalRequired {
                                                pending,
                                                evidence,
                                                impact: vec![],
                                            });
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    self.execute_and_handle(pending, on_event);
                } else {
                    // Multi-action transaction: skip per-file LSP pre-check.
                    self.execute_transaction(tx, on_event);
                }
            }
            PendingApprovalStage::PreCheckComplete(tx) => {
                if tx.is_single() {
                    self.execute_and_handle(tx.into_single(), on_event);
                } else {
                    self.execute_transaction(tx, on_event);
                }
            }
        }
    }

    fn execute_and_handle(
        &mut self,
        pending: PendingAction,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let tool_name = pending.tool_name.clone();
        on_event(RuntimeEvent::ActivityChanged(Activity::ExecutingTools {
            tool: short_tool_name(&tool_name).to_string(),
            detail: None,
        }));

        let mut before_syms: Vec<(String, String)> = Vec::new();
        let mut single_rel_path: Option<String> = None;
        if matches!(tool_name.as_str(), "edit_file" | "write_file") {
            if let Some(abs_path) = extract_absolute_path_from_payload(&pending.payload) {
                let before = std::fs::read_to_string(&abs_path).unwrap_or_default();
                if let Some(store) = &self.symbol_store {
                    if let Ok(rel) =
                        std::path::Path::new(&abs_path).strip_prefix(self.project_root.path())
                    {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        let root = self.project_root.path().to_string_lossy();
                        before_syms = store
                            .symbols_for_file(&root, &rel)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|s| (s.name, s.signature))
                            .collect();
                        single_rel_path = Some(rel);
                    }
                }
                self.undo_stack.push((abs_path, before));
                if self.undo_stack.len() > 5 {
                    self.undo_stack.remove(0);
                }
            }
        }

        // MCP tool execution — intercept before the registry. MCP tools are not
        // registered, so execute_approved() would return NotFound. Decode the payload
        // built by the tool_round intercept, call the server, commit the result, and
        // re-enter generation so the model synthesizes over it (read-only tools must
        // not take the mutation terminal-answer path). A tool-level error is surfaced
        // as committed tool output, not as a terminal failure.
        if tool_name.starts_with("mcp::") {
            let payload: serde_json::Value =
                serde_json::from_str(&pending.payload).unwrap_or_else(|_| serde_json::json!({}));
            let server = payload["server"].as_str().unwrap_or("").to_string();
            let bare_tool = payload["tool"].as_str().unwrap_or("").to_string();
            let args = payload["args"].clone();
            // Scope the &mut borrow of mcp_manager to this match so the owned result
            // releases it before commit_tool_results / run_turns re-borrow self.
            let call_result = match self.mcp_manager {
                Some(ref mut mcp) => mcp.call_tool(&server, &bare_tool, args),
                None => {
                    on_event(RuntimeEvent::SystemMessage(
                        "MCP tool called but no MCP manager configured.".to_string(),
                    ));
                    return;
                }
            };
            match call_result {
                Ok(result) => {
                    let output = ToolOutput::McpResult(result);
                    let summary = tool_codec::render_compact_summary(&output);
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: tool_name.clone(),
                        summary: Some(summary),
                    });
                    self.commit_tool_results(tool_codec::format_tool_result(&tool_name, &output));
                    on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                    self.run_turns(0, on_event);
                }
                Err(e) => {
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: tool_name.clone(),
                        summary: None,
                    });
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "MCP tool call failed: {e}"
                    )));
                }
            }
            return;
        }

        match self.registry.execute_approved(&pending) {
            Ok(output) => {
                self.invalidate_project_snapshot_if_needed(&output);
                if matches!(tool_name.as_str(), "edit_file" | "write_file") {
                    if let Some(abs_path) = extract_absolute_path_from_payload(&pending.payload) {
                        self.rebuild_index_for_file(std::path::Path::new(&abs_path), on_event);
                        if let Some(ref rel_path) = single_rel_path {
                            if let Some(store) = &self.symbol_store {
                                let root = self.project_root.path().to_string_lossy().to_string();
                                let after_recs: Vec<SymbolRecord> =
                                    store.symbols_for_file(&root, rel_path).unwrap_or_default();
                                let after_syms: Vec<(String, String)> = after_recs
                                    .iter()
                                    .map(|s| (s.name.clone(), s.signature.clone()))
                                    .collect();
                                if let Some((diff_msg, changed_names)) =
                                    command_handlers::diff_symbol_signatures(
                                        &before_syms,
                                        &after_syms,
                                    )
                                {
                                    let precise = if let Some(name) = changed_names.first() {
                                        if let Some(sym) =
                                            after_recs.iter().find(|r| &r.name == name)
                                        {
                                            if let Ok(src) = std::fs::read_to_string(&abs_path) {
                                                if let Ok(locs) = self.lsp.query_references(
                                                    std::path::Path::new(&abs_path),
                                                    &src,
                                                    sym.line,
                                                    sym.col,
                                                ) {
                                                    let project_root = self.project_root.path();
                                                    let filtered: Vec<String> = locs
                                                        .into_iter()
                                                        .filter_map(|loc| {
                                                            loc.path
                                                                .strip_prefix(project_root)
                                                                .ok()
                                                                .map(|rel| {
                                                                    format!(
                                                                        "  {}:{}",
                                                                        rel.display(),
                                                                        loc.line
                                                                    )
                                                                })
                                                        })
                                                        .collect();
                                                    if !filtered.is_empty() {
                                                        let total = filtered.len();
                                                        let capped: Vec<String> =
                                                            filtered.into_iter().take(10).collect();
                                                        let header = if total > 10 {
                                                            format!(
                                                                "{total} call sites (showing 10):"
                                                            )
                                                        } else {
                                                            format!("{total} call sites:")
                                                        };
                                                        Some(format!(
                                                            "{header}\n{}",
                                                            capped.join("\n")
                                                        ))
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    None
                                                }
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    on_event(RuntimeEvent::SystemMessage(
                                        precise.unwrap_or(diff_msg),
                                    ));
                                }
                            }
                        }
                    }
                }
                let summary = tool_codec::render_compact_summary(&output);
                let final_answer = mutation_complete_final_answer(&tool_name, &summary);
                on_event(RuntimeEvent::ToolCallFinished {
                    name: tool_name.clone(),
                    summary: Some(summary),
                });
                self.commit_tool_results(tool_codec::format_tool_result(&tool_name, &output));
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                // Git branch switch: skip verify and LSP blocks (not applicable to git ops).
                // Reset session state — the branch has changed so all path anchors, conversation
                // history, and mutation tracking are now stale. git checkout itself refuses to
                // switch with uncommitted conflicting changes, so no pre-check is needed here.
                if tool_name == "git_branch_switch" {
                    self.undo_stack.clear();
                    self.active_ability = None;
                    self.prompt_physics.active_ability = None;
                    self.active_skill = None;
                    self.prompt_physics.active_skill = None;
                    self.correction_attempts = 0;
                    self.project_snapshot_cache = ProjectStructureSnapshotCache::default();
                    self.handle_reset(on_event);
                    return;
                }
                if matches!(tool_name.as_str(), "edit_file" | "write_file") && self.lsp.is_enabled()
                {
                    if let Some(abs_path) = extract_absolute_path_from_payload(&pending.payload) {
                        let ext = std::path::Path::new(&abs_path)
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("");
                        if self.lsp.config().extensions.contains(&ext.to_string()) {
                            if let Ok(source) = std::fs::read_to_string(&abs_path) {
                                if let Ok(diagnostics) = self
                                    .lsp
                                    .query_diagnostics(std::path::Path::new(&abs_path), &source)
                                {
                                    if !diagnostics.is_empty() {
                                        let diag_text = diagnostics
                                            .iter()
                                            .map(|d| {
                                                format!(
                                                    "[{}] line {}:{} {}: {}",
                                                    d.severity,
                                                    d.line,
                                                    d.column,
                                                    d.source.as_deref().unwrap_or("rust-analyzer"),
                                                    d.message
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        trace_runtime_decision(
                                            on_event,
                                            "lsp_diagnostics_injected",
                                            &[
                                                ("path", abs_path.clone()),
                                                ("count", diagnostics.len().to_string()),
                                            ],
                                        );
                                        self.commit_tool_results(format!(
                                            "\n=== lsp_diagnostics: {} ===\n{}\n=== /lsp_diagnostics ===\n",
                                            abs_path, diag_text
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                // Runtime-initiated verify command: not a model-proposed mutation, not subject
                // to the approval gate. Uses std::process::Command directly (not ShellTool or
                // registry.execute_approved) because this is a read-only verification step
                // initiated by the runtime after an approved mutation, not a user action.
                // When deferred_verify is true, verification runs on a background thread after
                // the turn completes (display-only). The correction loop requires sync verify.
                if !self.deferred_verify && matches!(tool_name.as_str(), "edit_file" | "write_file")
                {
                    if let Some(verify_cmd) = self.verify_command.clone() {
                        if let Some(abs_path) = extract_absolute_path_from_payload(&pending.payload)
                        {
                            if let Some(combined) = self.run_verify_command(&verify_cmd, on_event) {
                                if self.max_correction_attempts > 0
                                    && self.correction_attempts < self.max_correction_attempts
                                {
                                    // Correction attempt: inject a correction prompt and
                                    // re-enter the turn loop. The [runtime:correction]
                                    // prefix is mandatory — it suppresses TurnContext
                                    // surface/intent re-classification (engine.rs ~line 1641).
                                    self.correction_attempts += 1;
                                    on_event(RuntimeEvent::SystemMessage(format!(
                                        "{verify_cmd}: failed — requesting correction \
                                         (attempt {}/{})",
                                        self.correction_attempts, self.max_correction_attempts
                                    )));
                                    let correction_prompt = format!(
                                        "[runtime:correction] {verify_cmd} failed after \
                                         editing {}:\n{}\n\nEmit a corrective \
                                         [edit_file: ...] that fixes the error. \
                                         Do not include any other content.",
                                        abs_path,
                                        combined.trim()
                                    );
                                    self.conversation.push_user(correction_prompt);
                                    on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                                    self.run_turns(0, on_event);
                                    if self.pending_action.is_some() {
                                        // Corrective edit is pending approval — suspend
                                        // here and let the next Approve call continue.
                                        return;
                                    }
                                    // Model responded with prose instead of an edit.
                                    // run_turns already called finish_with_runtime_answer
                                    // for the prose answer, so we must not call it again.
                                    on_event(RuntimeEvent::SystemMessage(format!(
                                        "{verify_cmd}: failed after {} correction \
                                         attempt(s) — manual fix required\n{}",
                                        self.correction_attempts,
                                        combined.trim()
                                    )));
                                    self.correction_attempts = 0;
                                    return;
                                } else {
                                    // Corrections disabled or max attempts reached.
                                    on_event(RuntimeEvent::SystemMessage(format!(
                                        "{verify_cmd}: failed after {} correction \
                                         attempt(s) — manual fix required\n{}",
                                        self.correction_attempts,
                                        combined.trim()
                                    )));
                                    self.correction_attempts = 0;
                                }
                            }
                        }
                    }
                }
                self.finish_with_runtime_answer(
                    &final_answer,
                    AnswerSource::ToolAssisted { rounds: 1 },
                    on_event,
                );
                if matches!(tool_name.as_str(), "edit_file" | "write_file") {
                    let test_cmd = self.config.project.test_command.clone();
                    if let Some(cmd) = test_cmd {
                        let input = ToolInput::Shell { command: cmd };
                        if let Ok(resolved) = resolve(&self.project_root, &input) {
                            match self.registry.dispatch(resolved) {
                                Ok(ToolRunResult::Approval(pending)) => {
                                    self.pending_action =
                                        Some(PendingApprovalStage::AwaitingPreCheck(
                                            PendingTransaction::single(pending.clone()),
                                        ));
                                    on_event(RuntimeEvent::ApprovalRequired {
                                        pending,
                                        evidence: vec![],
                                        impact: vec![],
                                    });
                                }
                                Ok(ToolRunResult::Immediate(output)) => {
                                    self.invalidate_project_snapshot_if_needed(&output);
                                    self.commit_tool_results(tool_codec::format_tool_result(
                                        "shell", &output,
                                    ));
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }
            }
            Err(e) => {
                on_event(RuntimeEvent::ToolCallFinished {
                    name: tool_name.clone(),
                    summary: None,
                });
                let error_text = tool_codec::format_tool_error(&tool_name, &e.to_string());
                self.conversation.push_user(error_text);
                // On failure, let the model respond — it may want to retry.
                on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                self.run_turns(0, on_event);
            }
        }
    }

    /// Executes a multi-action transaction atomically:
    /// 1. Captures pre-edit snapshots for all files (best-effort — no ACID guarantee).
    /// 2. Executes each action in order; rolls back all prior edits on any failure.
    /// 3. Runs verify_command after all edits complete if configured.
    ///    Correction loop is intentionally skipped for transactions — it applies to
    ///    single-edit mutations only.
    fn execute_transaction(
        &mut self,
        tx: PendingTransaction,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        on_event(RuntimeEvent::ActivityChanged(Activity::ExecutingTools {
            tool: short_tool_name(&tx.first().tool_name).to_string(),
            detail: None,
        }));

        // Step 1: Capture pre-edit state for rollback.
        // Files that do not exist yet (write_file creating a new file) get an empty snapshot;
        // restoring them is a no-op if the write was the first action to fail.
        let mut snapshots: Vec<(String, String)> = Vec::new();
        let mut before_syms_map: std::collections::HashMap<String, Vec<(String, String)>> =
            std::collections::HashMap::new();
        for action in &tx.actions {
            if matches!(action.tool_name.as_str(), "edit_file" | "write_file") {
                if let Some(abs_path) = extract_absolute_path_from_payload(&action.payload) {
                    let before = std::fs::read_to_string(&abs_path).unwrap_or_default();
                    if let Some(store) = &self.symbol_store {
                        if let Ok(rel) =
                            std::path::Path::new(&abs_path).strip_prefix(self.project_root.path())
                        {
                            let rel = rel.to_string_lossy().replace('\\', "/");
                            let root = self.project_root.path().to_string_lossy();
                            let syms: Vec<(String, String)> = store
                                .symbols_for_file(&root, &rel)
                                .unwrap_or_default()
                                .into_iter()
                                .map(|s| (s.name, s.signature))
                                .collect();
                            before_syms_map.insert(rel, syms);
                        }
                    }
                    snapshots.push((abs_path, before));
                }
            }
        }

        // Step 2: Execute all actions; roll back on first failure.
        let mut results = String::new();
        let mut all_ok = true;
        let mut failed_name = String::new();
        let mut failed_error = String::new();
        let mut executed_count = 0usize;

        for action in &tx.actions {
            match self.registry.execute_approved(action) {
                Ok(output) => {
                    self.invalidate_project_snapshot_if_needed(&output);
                    if matches!(action.tool_name.as_str(), "edit_file" | "write_file") {
                        if let Some(abs_path) = extract_absolute_path_from_payload(&action.payload)
                        {
                            self.rebuild_index_for_file(std::path::Path::new(&abs_path), on_event);
                            if let Ok(rel) = std::path::Path::new(&abs_path)
                                .strip_prefix(self.project_root.path())
                            {
                                let rel = rel.to_string_lossy().replace('\\', "/");
                                if let Some(store) = &self.symbol_store {
                                    let root =
                                        self.project_root.path().to_string_lossy().to_string();
                                    let after_recs: Vec<SymbolRecord> =
                                        store.symbols_for_file(&root, &rel).unwrap_or_default();
                                    let after_syms: Vec<(String, String)> = after_recs
                                        .iter()
                                        .map(|s| (s.name.clone(), s.signature.clone()))
                                        .collect();
                                    let before =
                                        before_syms_map.get(&rel).cloned().unwrap_or_default();
                                    if let Some((diff_msg, changed_names)) =
                                        command_handlers::diff_symbol_signatures(
                                            &before,
                                            &after_syms,
                                        )
                                    {
                                        let precise = if let Some(name) = changed_names.first() {
                                            if let Some(sym) =
                                                after_recs.iter().find(|r| &r.name == name)
                                            {
                                                if let Ok(src) = std::fs::read_to_string(&abs_path)
                                                {
                                                    if let Ok(locs) = self.lsp.query_references(
                                                        std::path::Path::new(&abs_path),
                                                        &src,
                                                        sym.line,
                                                        sym.col,
                                                    ) {
                                                        let project_root = self.project_root.path();
                                                        let filtered: Vec<String> = locs
                                                            .into_iter()
                                                            .filter_map(|loc| {
                                                                loc.path
                                                                    .strip_prefix(project_root)
                                                                    .ok()
                                                                    .map(|r| {
                                                                        format!(
                                                                            "  {}:{}",
                                                                            r.display(),
                                                                            loc.line
                                                                        )
                                                                    })
                                                            })
                                                            .collect();
                                                        if !filtered.is_empty() {
                                                            let total = filtered.len();
                                                            let capped: Vec<String> = filtered
                                                                .into_iter()
                                                                .take(10)
                                                                .collect();
                                                            let header = if total > 10 {
                                                                format!(
                                                                    "{total} call sites (showing 10):"
                                                                )
                                                            } else {
                                                                format!("{total} call sites:")
                                                            };
                                                            Some(format!(
                                                                "{header}\n{}",
                                                                capped.join("\n")
                                                            ))
                                                        } else {
                                                            None
                                                        }
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    None
                                                }
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        };
                                        on_event(RuntimeEvent::SystemMessage(
                                            precise.unwrap_or(diff_msg),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    let summary = tool_codec::render_compact_summary(&output);
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: action.tool_name.clone(),
                        summary: Some(summary.clone()),
                    });
                    results.push_str(&tool_codec::format_tool_result(&action.tool_name, &output));
                    executed_count += 1;
                }
                Err(e) => {
                    on_event(RuntimeEvent::ToolCallFinished {
                        name: action.tool_name.clone(),
                        summary: None,
                    });
                    all_ok = false;
                    failed_name = action.tool_name.clone();
                    failed_error = e.to_string();
                    break;
                }
            }
        }

        if !all_ok {
            // Roll back all successfully executed edits in reverse order.
            // This is best-effort: filesystem errors during rollback are silently ignored.
            for (path, before) in snapshots[..executed_count].iter().rev() {
                let _ = std::fs::write(path, before);
            }
            on_event(RuntimeEvent::SystemMessage(format!(
                "transaction failed on {}: {} — rolled back {} edit(s)",
                failed_name, failed_error, executed_count
            )));
            self.finish_with_runtime_answer(
                "Transaction rolled back.",
                AnswerSource::ToolAssisted { rounds: 1 },
                on_event,
            );
            return;
        }

        // All edits succeeded — push pre-edit states to undo stack for /undo support.
        for (abs_path, before) in snapshots {
            self.undo_stack.push((abs_path, before));
            if self.undo_stack.len() > 5 {
                self.undo_stack.remove(0);
            }
        }

        if !results.is_empty() {
            self.commit_tool_results(results);
            self.conversation
                .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
        }

        let n = tx.actions.len();
        let final_answer = format!("{n} edit(s) applied successfully.");

        // Step 3: Run verify_command if configured.
        // Correction loop is intentionally skipped for transactions.
        // When deferred_verify is true, the background thread handles this instead.
        if !self.deferred_verify {
            if let Some(verify_cmd) = self.verify_command.clone() {
                if let Some(combined) = self.run_verify_command(&verify_cmd, on_event) {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "{verify_cmd}: failed after transaction — manual fix required\n{}",
                        combined.trim()
                    )));
                }
            }
        }

        self.finish_with_runtime_answer(
            &final_answer,
            AnswerSource::ToolAssisted { rounds: 1 },
            on_event,
        );
    }

    fn handle_transaction_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        match &self.pending_action {
            Some(stage) => {
                let tx = match stage {
                    PendingApprovalStage::AwaitingPreCheck(tx)
                    | PendingApprovalStage::PreCheckComplete(tx) => tx,
                };
                if tx.is_single() {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "pending: 1 action — {}",
                        tx.first().summary
                    )));
                } else {
                    let files: Vec<String> = tx
                        .actions
                        .iter()
                        .map(|a| {
                            extract_absolute_path_from_payload(&a.payload)
                                .unwrap_or_else(|| a.tool_name.clone())
                        })
                        .collect();
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "pending transaction: {} action(s)\n{}",
                        tx.actions.len(),
                        files.join("\n")
                    )));
                }
            }
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "no pending transaction".to_string(),
                ));
            }
        }
    }

    fn handle_reject(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let tx = match self.pending_action.take() {
            Some(stage) => stage.into_transaction(),
            None => {
                on_event(RuntimeEvent::Failed {
                    message: "No pending action to reject.".to_string(),
                });
                return;
            }
        };

        // Fire ToolCallFinished for all actions (matching ToolCallStarted fired during proposal).
        for action in &tx.actions {
            on_event(RuntimeEvent::ToolCallFinished {
                name: action.tool_name.clone(),
                summary: None,
            });
        }
        let tool_name = tx.first().tool_name.clone();
        let rejection = tool_codec::format_tool_error(
            &tool_name,
            "user rejected this action — do not retry or re-propose it. \
             Acknowledge the cancellation in plain text and wait for the user's next instruction.",
        );
        self.conversation.push_user(rejection);
        self.finish_with_runtime_answer(
            rejection_final_answer(&tool_name),
            AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::RejectedMutation,
                rounds: 1,
            },
            on_event,
        );
    }

    /// Returns the list of files that import the mutation target from the pending approval.
    /// Only meaningful for edit_file and write_file; returns empty for all others.
    /// Query failure degrades silently — never surfaces as Failed.
    fn impact_for_pending(&self, pending: &PendingAction) -> Vec<String> {
        if !matches!(pending.tool_name.as_str(), "edit_file" | "write_file") {
            return vec![];
        }
        let Some(store) = &self.symbol_store else {
            return vec![];
        };
        // Payload format: v2\x00{absolute_path}\x00...
        let parts: Vec<&str> = pending.payload.splitn(3, '\x00').collect();
        if parts.len() < 2 {
            return vec![];
        }
        let abs_path = std::path::Path::new(parts[1]);
        let rel_path = match abs_path.strip_prefix(self.project_root.path()) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => return vec![],
        };
        let project_root = self.project_root.path().to_string_lossy();
        store
            .importers_of(&project_root, &rel_path, 10)
            .unwrap_or_default()
    }

    /// Runs the generate -> tool-round loop until the model produces a final answer,
    /// the tool round limit is reached, or a tool action requires approval.
    /// `tool_rounds` is the count already consumed before this call (0 for a fresh turn).
    fn run_turns(&mut self, tool_rounds: usize, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.run_turns_with_initial_reads(tool_rounds, HashSet::new(), false, on_event);
    }

    fn run_turns_with_initial_reads(
        &mut self,
        tool_rounds: usize,
        reads_this_turn: HashSet<String>,
        start_in_post_read_answer_phase: bool,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let Ok(ctx) = TurnContext::build(self, tool_rounds, &reads_this_turn, on_event) else {
            return;
        };
        let mut state = TurnState::new(
            tool_rounds,
            reads_this_turn,
            start_in_post_read_answer_phase,
            self.pending_runtime_call.take(),
            self.backend.capabilities().context_window_tokens,
        );
        if let Some(ref store) = self.symbol_store {
            let root_str = self.project_root.path().to_string_lossy().into_owned();
            if store.import_count(&root_str).unwrap_or(0) > 0 {
                if let Ok(edges) = store.all_imports(&root_str) {
                    for edge in &edges {
                        state
                            .investigation
                            .graph
                            .record_import_edge(&edge.from_file, &edge.to_file);
                    }
                }
            }
        }
        state.investigation.investigation_depth = self.investigation_depth;
        seed_pending_runtime_call(&ctx, &mut state);
        loop {
            match self.run_loop_body(&ctx, &mut state, on_event) {
                TurnSignal::Finish => {
                    state.turn_perf.emit_summary(on_event);
                    self.maybe_warn_or_prune_context(&state.turn_perf, on_event);
                    self.write_retrieval_log(&ctx, &state, on_event);
                    return;
                }
                TurnSignal::Continue => continue,
                TurnSignal::Suspend => return,
            }
        }
    }

    fn run_loop_body(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> TurnSignal {
        let effective_surface = if state.answer_phase.is_some() {
            ToolSurface::AnswerOnly
        } else {
            ctx.tool_surface
        };
        if matches!(effective_surface, ToolSurface::AnswerOnly) {
            trace_runtime_decision(
                on_event,
                "answer_phase_synthesis_bounded",
                &[("surface", "AnswerOnly".into())],
            );
        }
        let is_correction_round = !matches!(
            state.next_round_cause,
            GenerationRoundCause::Initial
                | GenerationRoundCause::ToolResults
                | GenerationRoundCause::ReadRequestToolRequired
                | GenerationRoundCause::ReadBeforeAnsweringCorrection
        );
        let project_snapshot_hint = if state.pending_runtime_call.is_none() && !is_correction_round
        {
            self.maybe_render_project_snapshot_hint(effective_surface)
        } else {
            None
        };
        let test_coverage_hint = if state.pending_runtime_call.is_none() && !is_correction_round {
            self.maybe_render_test_coverage_hint(effective_surface)
        } else {
            None
        };
        let prompt_chars = if state.turn_perf.is_enabled() {
            estimate_generation_prompt_chars(
                &self.conversation,
                effective_surface,
                project_snapshot_hint.as_deref(),
                test_coverage_hint.as_deref(),
            )
        } else {
            0
        };

        state.turn_perf.start_round(
            state.next_round_label,
            state.next_round_cause,
            prompt_chars,
            on_event,
        );

        let (calls, response, seeded_pre_generation) = if let Some(pending) =
            state.pending_runtime_call.take()
        {
            (vec![pending.input], None, pending.seeded_pre_generation)
        } else {
            let response = {
                let recall_facts: Vec<crate::storage::memory::MemoryFact> =
                    if self.config.memory.enabled {
                        if let Some(ref mgr) = self.memory_manager {
                            let query = self.conversation.last_user_content().unwrap_or("");
                            let root_str = self.project_root.path().to_string_lossy().into_owned();
                            mgr.recall(query, Some(&root_str), self.config.memory.recall_top_k)
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };
                let mut perf_on_event = |event| {
                    if let RuntimeEvent::BackendTiming { stage, elapsed_ms } = &event {
                        state.turn_perf.record_backend_timing(*stage, *elapsed_ms);
                    }
                    if let RuntimeEvent::BackendTokenCounts { prompt, completion } = &event {
                        state.turn_perf.record_token_counts(*prompt, *completion);
                    }
                    on_event(event);
                };

                let dynamic_tool_names: Vec<&str> = self
                    .discovered_tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect();
                match run_generate_turn(
                    self.backend.as_mut(),
                    &mut self.conversation,
                    effective_surface,
                    project_snapshot_hint.as_deref(),
                    test_coverage_hint.as_deref(),
                    ctx.investigation_mode,
                    &self.prompt_physics,
                    self.constrained_output,
                    &recall_facts,
                    &dynamic_tool_names,
                    &mut perf_on_event,
                ) {
                    Ok(Some(r)) => r,
                    Ok(None) => {
                        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                        on_event(RuntimeEvent::Failed {
                            message: format!("{} returned no output.", self.backend.name()),
                        });
                        return TurnSignal::Finish;
                    }
                    Err(e) => {
                        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                        on_event(RuntimeEvent::Failed {
                            message: e.to_string(),
                        });
                        return TurnSignal::Finish;
                    }
                }
            };

            let dynamic_names: Vec<&str> = self
                .discovered_tools
                .iter()
                .map(|t| t.name.as_str())
                .collect();
            let calls = tool_codec::parse_all_tool_inputs(&response, &dynamic_names);
            (calls, Some(response), false)
        };

        if let Some(signal) =
            self.check_tool_call_gates(ctx, state, &calls, response.as_deref(), on_event)
        {
            return signal;
        }

        if calls.is_empty() {
            let response = response.expect("response exists when calls are empty");
            return self.handle_no_tool_call(ctx, state, response, seeded_pre_generation, on_event);
        }

        self.dispatch_tool_round(ctx, state, calls, seeded_pre_generation, on_event)
    }

    fn dispatch_tool_round(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        calls: Vec<ToolInput>,
        seeded_pre_generation: bool,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> TurnSignal {
        if !seeded_pre_generation {
            state.tool_rounds += 1;

            if state.tool_rounds >= MAX_TOOL_ROUNDS {
                on_event(RuntimeEvent::AnswerReady(AnswerSource::ToolLimitReached));
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                return TurnSignal::Finish;
            }
        }

        on_event(RuntimeEvent::ActivityChanged(tool_input_activity(
            calls.first(),
        )));
        let t_tool_start = if state.turn_perf.is_enabled() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        let dynamic_allowed: std::collections::HashSet<String> = self
            .discovered_tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        match run_tool_round(
            &self.project_root,
            &self.registry,
            calls,
            &mut state.last_call_key,
            &mut state.search_budget,
            &mut state.investigation,
            &mut self.lsp,
            &mut state.reads_this_turn,
            &mut self.anchors,
            ctx.tool_surface,
            &mut state.disallowed_tool_attempts,
            &mut state.weak_search_query_attempts,
            ctx.mutation_allowed,
            ctx.investigation_required,
            ctx.investigation_mode,
            ctx.requested_read_path.as_deref(),
            &mut state.requested_read_completed,
            ctx.investigation_path_scope.as_deref(),
            self.symbol_store.as_ref(),
            self.embedding_provider.as_deref(),
            &self.retrieval_config,
            &dynamic_allowed,
            on_event,
        ) {
            ToolRoundOutcome::Completed {
                results,
                git_acquisition_answer,
            } => {
                if seeded_pre_generation {
                    state.seeded_tool_executed = true;
                    state.last_call_key = None;
                    if matches!(
                        ctx.retrieval_intent,
                        RetrievalIntent::DirectoryListing { .. }
                    ) {
                        state.answer_phase = Some(AnswerPhaseKind::PostRead);
                    }
                    // Invariant: ctx.requested_read_path.is_some() identifies a DirectRead turn.
                    // Capture the result now (before commit moves it) so the runtime can
                    // serve it as a deterministic fallback if model synthesis loops.
                    if ctx.requested_read_path.is_some() {
                        state.direct_read_result = Some(results.clone());
                        if matches!(ctx.direct_read_mode, Some(DirectReadMode::Explain)) {
                            state.answer_phase = Some(AnswerPhaseKind::PostRead);
                        }
                    }
                }
                if let Some(t) = t_tool_start {
                    state
                        .turn_perf
                        .record_tool_elapsed(t.elapsed().as_millis() as u64);
                }
                if seeded_pre_generation
                    && matches!(ctx.direct_read_mode, Some(DirectReadMode::Raw))
                {
                    let answer = direct_read_fallback_answer(&results);
                    self.commit_tool_results(results);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                    self.finish_with_runtime_answer(
                        &answer,
                        AnswerSource::ToolAssisted { rounds: 1 },
                        on_event,
                    );
                    on_event(RuntimeEvent::DirectReadCompleted);
                    return TurnSignal::Finish;
                }
                let post_tool_cause = infer_post_tool_round_cause(&results);
                self.commit_tool_results(results);
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                if ctx.tool_surface == ToolSurface::GitReadOnly {
                    if let Some(answer) = git_acquisition_answer {
                        trace_runtime_decision(
                            on_event,
                            "git_acquisition_completed",
                            &[("rounds", state.tool_rounds.to_string())],
                        );
                        self.finish_with_runtime_answer(
                            &answer,
                            AnswerSource::ToolAssisted {
                                rounds: state.tool_rounds,
                            },
                            on_event,
                        );
                        return TurnSignal::Finish;
                    }
                }
                if state.answer_phase.is_none() {
                    if ctx.investigation_required && state.investigation.evidence_ready() {
                        state.answer_phase = Some(AnswerPhaseKind::InvestigationEvidenceReady);
                    } else if !ctx.investigation_required
                        && !ctx.mutation_allowed
                        && !state.reads_this_turn.is_empty()
                    {
                        state.answer_phase = Some(AnswerPhaseKind::PostRead);
                    }
                }
                state.next_round_label = GenerationRoundLabel::PostTool;
                state.next_round_cause = post_tool_cause;
                // Signal re-entry before the next generate so the status bar
                // transitions cleanly from "executing tools" → "processing" → …
                on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
                // Do not return — loop continues so the model is re-invoked
                // with the tool results in context to produce a synthesis response.
            }
            ToolRoundOutcome::TerminalAnswer {
                results,
                answer,
                reason,
            } => {
                if let Some(t) = t_tool_start {
                    state
                        .turn_perf
                        .record_tool_elapsed(t.elapsed().as_millis() as u64);
                }
                self.commit_tool_results(results);
                self.conversation
                    .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                self.finish_with_runtime_answer(
                    &answer,
                    AnswerSource::RuntimeTerminal {
                        reason,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return TurnSignal::Finish;
            }
            ToolRoundOutcome::ApprovalRequired {
                accumulated,
                pending,
            } => {
                if let Some(t) = t_tool_start {
                    state
                        .turn_perf
                        .record_tool_elapsed(t.elapsed().as_millis() as u64);
                }
                if !accumulated.is_empty() {
                    self.commit_tool_results(accumulated);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                }
                self.pending_action = Some(PendingApprovalStage::AwaitingPreCheck(
                    PendingTransaction::single(pending.clone()),
                ));
                let evidence = state.investigation.evidence_summary();
                let impact = self.impact_for_pending(&pending);
                on_event(RuntimeEvent::ApprovalRequired {
                    pending,
                    evidence,
                    impact,
                });
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                return TurnSignal::Finish;
            }
            ToolRoundOutcome::TransactionRequired {
                accumulated,
                actions,
            } => {
                if let Some(t) = t_tool_start {
                    state
                        .turn_perf
                        .record_tool_elapsed(t.elapsed().as_millis() as u64);
                }
                if !accumulated.is_empty() {
                    self.commit_tool_results(accumulated);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                }
                self.pending_action =
                    Some(PendingApprovalStage::AwaitingPreCheck(PendingTransaction {
                        actions: actions.clone(),
                    }));
                let evidence = state.investigation.evidence_summary();
                on_event(RuntimeEvent::TransactionApprovalRequired {
                    actions,
                    evidence,
                    impact: vec![],
                });
                on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
                return TurnSignal::Finish;
            }
            ToolRoundOutcome::RuntimeDispatch { accumulated, call } => {
                if let Some(t) = t_tool_start {
                    state
                        .turn_perf
                        .record_tool_elapsed(t.elapsed().as_millis() as u64);
                }
                if !accumulated.is_empty() {
                    self.commit_tool_results(accumulated);
                    self.conversation
                        .trim_tool_exchanges_if_needed(self.context_policy.trim_threshold);
                }
                state.pending_runtime_call = Some(PendingRuntimeCall {
                    input: call,
                    seeded_pre_generation: false,
                });
                on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
            }
        }
        TurnSignal::Continue
    }

    fn handle_no_tool_call(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        response: String,
        _seeded_pre_generation: bool,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> TurnSignal {
        if let Some(s) = self.check_correction_echo(ctx, state, &response, on_event) {
            return s;
        }
        if let Some(s) = self.check_protocol_violations(state, &response, on_event) {
            return s;
        }
        if let Some(s) = self.check_evidence_and_admission_gates(ctx, state, &response, on_event) {
            return s;
        }
        let source = if state.tool_rounds == 0 {
            if state.seeded_tool_executed {
                AnswerSource::ToolAssisted { rounds: 1 }
            } else {
                AnswerSource::Direct
            }
        } else {
            AnswerSource::ToolAssisted {
                rounds: state.tool_rounds,
            }
        };
        emit_visible_assistant_message(&response, on_event);
        on_event(RuntimeEvent::AnswerReady(source));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
        TurnSignal::Finish
    }
    fn check_tool_call_gates(
        &mut self,
        ctx: &TurnContext,
        state: &mut TurnState,
        calls: &[ToolInput],
        response: Option<&str>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<TurnSignal> {
        if let Some(phase) = state.answer_phase {
            if !calls.is_empty() && response.is_some() {
                state.post_answer_phase_tool_attempts += 1;
                if matches!(phase, AnswerPhaseKind::InvestigationEvidenceReady) {
                    trace_runtime_decision(
                        on_event,
                        "post_evidence_tool_call_rejected",
                        &[
                            (
                                "attempts",
                                state.post_answer_phase_tool_attempts.to_string(),
                            ),
                            ("tool_count", calls.len().to_string()),
                        ],
                    );
                }
                self.conversation.discard_last_if_assistant();
                if state.post_answer_phase_tool_attempts == 1 {
                    let (label, cause) = match phase {
                        AnswerPhaseKind::PostRead => (
                            GenerationRoundLabel::CorrectionRetry,
                            GenerationRoundCause::AnswerPhaseToolCallRejected,
                        ),
                        AnswerPhaseKind::InvestigationEvidenceReady => (
                            GenerationRoundLabel::PostEvidenceRetry,
                            GenerationRoundCause::PostEvidenceToolCallRejected,
                        ),
                    };
                    state.next_round_label = label;
                    state.next_round_cause = cause;
                    self.conversation.push_user(
                        match phase {
                            AnswerPhaseKind::PostRead => TURN_COMPLETE_ANSWER_ONLY,
                            AnswerPhaseKind::InvestigationEvidenceReady => {
                                EVIDENCE_READY_ANSWER_ONLY
                            }
                        }
                        .to_string(),
                    );
                    return Some(TurnSignal::Continue);
                }
                let (answer, reason): (String, RuntimeTerminalReason) = match phase {
                    AnswerPhaseKind::PostRead => {
                        let answer = if matches!(ctx.direct_read_mode, Some(DirectReadMode::Raw)) {
                            state
                                .direct_read_result
                                .as_deref()
                                .map(direct_read_fallback_answer)
                                .unwrap_or_else(|| {
                                    repeated_tool_after_answer_phase_final_answer().to_string()
                                })
                        } else {
                            repeated_tool_after_answer_phase_final_answer().to_string()
                        };
                        (answer, RuntimeTerminalReason::RepeatedToolAfterAnswerPhase)
                    }
                    AnswerPhaseKind::InvestigationEvidenceReady => (
                        repeated_tool_after_evidence_ready_final_answer().to_string(),
                        RuntimeTerminalReason::RepeatedToolAfterEvidenceReady,
                    ),
                };
                self.finish_with_runtime_answer(
                    &answer,
                    AnswerSource::RuntimeTerminal {
                        reason,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return Some(TurnSignal::Finish);
            }
        }

        if state.search_budget.is_closed()
            && calls
                .iter()
                .any(|c| matches!(c, ToolInput::SearchCode { .. }))
        {
            if state.search_budget.empty_retry_exhausted()
                && !state.investigation.search_produced_results()
                && state.investigation.files_read_count() == 0
            {
                trace_insufficient_evidence_terminal(
                    "empty_search_retry_exhausted",
                    state.tool_rounds,
                    &state.search_budget,
                    &state.investigation,
                    on_event,
                );
                self.conversation.discard_last_if_assistant();
                self.finish_with_runtime_answer(
                    insufficient_evidence_final_answer(),
                    AnswerSource::RuntimeTerminal {
                        reason: RuntimeTerminalReason::InsufficientEvidence,
                        rounds: state.tool_rounds,
                    },
                    on_event,
                );
                return Some(TurnSignal::Finish);
            }
            state.escalation.closed_search_budget_violations += 1;
            self.conversation.discard_last_if_assistant();
            if state.escalation.closed_search_budget_violations == 1 {
                self.conversation
                    .push_user(state.search_budget.closed_message().to_string());
                state.next_round_label = GenerationRoundLabel::CorrectionRetry;
                state.next_round_cause = GenerationRoundCause::SearchBudgetClosedCorrection;
                return Some(TurnSignal::Continue);
            }
            self.finish_with_runtime_answer(
                repeated_search_budget_violation_final_answer(),
                AnswerSource::RuntimeTerminal {
                    reason: RuntimeTerminalReason::RepeatedSearchBudgetViolation,
                    rounds: state.tool_rounds,
                },
                on_event,
            );
            return Some(TurnSignal::Finish);
        }

        None
    }

    pub(super) fn finish_with_runtime_answer(
        &mut self,
        answer: &str,
        source: AnswerSource,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        on_event(RuntimeEvent::ActivityChanged(Activity::Responding));
        self.conversation.begin_assistant_reply();
        on_event(RuntimeEvent::AssistantMessageStarted);
        self.conversation.push_assistant_chunk(answer);
        on_event(RuntimeEvent::AssistantMessageChunk(answer.to_string()));
        on_event(RuntimeEvent::AssistantMessageFinished);
        on_event(RuntimeEvent::AnswerReady(source));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    #[cfg(test)]
    pub(crate) fn set_pending_for_test(&mut self, action: PendingAction) {
        self.pending_action = Some(PendingApprovalStage::AwaitingPreCheck(
            PendingTransaction::single(action),
        ));
    }

    #[cfg(test)]
    pub(crate) fn project_snapshot_for_test(
        &mut self,
    ) -> std::io::Result<ProjectStructureSnapshot> {
        self.get_or_build_project_snapshot().cloned()
    }

    /// Runs `cmd` in the project root. Returns `None` on success or spawn error (both already
    /// emitted via `on_event`). Returns `Some(combined_output)` on non-zero exit so the caller
    /// can build a failure message; on success also resets `correction_attempts`.
    fn run_verify_command(
        &mut self,
        cmd: &str,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Option<String> {
        let mut cmd_parts = cmd.split_whitespace();
        let program = cmd_parts.next()?;
        let args: Vec<&str> = cmd_parts.collect();
        let is_cargo = program == "cargo";
        let is_ruff = program == "ruff";
        let final_args: Vec<&str> = if is_cargo {
            let mut a = args.clone();
            a.push("--message-format=json");
            a
        } else if is_ruff {
            let mut a = args.clone();
            a.push("--output-format=json");
            a
        } else {
            args.clone()
        };
        on_event(RuntimeEvent::SystemMessage("verifying...".to_string()));
        match std::process::Command::new(program)
            .args(&final_args)
            .current_dir(self.project_root.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(out) => {
                let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                if combined.len() > 4000 {
                    let boundary = combined
                        .char_indices()
                        .map(|(i, _)| i)
                        .filter(|&i| i <= 4000)
                        .last()
                        .unwrap_or(0);
                    combined.truncate(boundary);
                }
                let output_for_correction = if is_cargo {
                    let diagnostics = crate::runtime::diagnostics::parse_diagnostics(&combined);
                    if diagnostics.is_empty() {
                        on_event(RuntimeEvent::SystemMessage(format!("{cmd}: ok")));
                        self.correction_attempts = 0;
                        return None;
                    }
                    crate::runtime::diagnostics::format_diagnostics(&diagnostics)
                } else if is_ruff {
                    let diagnostics =
                        crate::runtime::diagnostics::parse_ruff_diagnostics(&combined);
                    if diagnostics.is_empty() {
                        on_event(RuntimeEvent::SystemMessage(format!("{cmd}: ok")));
                        self.correction_attempts = 0;
                        return None;
                    }
                    crate::runtime::diagnostics::format_diagnostics(&diagnostics)
                } else {
                    if combined.trim().is_empty() {
                        on_event(RuntimeEvent::SystemMessage(format!("{cmd}: ok")));
                        self.correction_attempts = 0;
                        return None;
                    }
                    combined
                };
                Some(output_for_correction)
            }
            Err(_) => {
                on_event(RuntimeEvent::SystemMessage(format!("{cmd}: unavailable")));
                None
            }
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.lsp.shutdown();
        if let Some(ref mut mcp) = self.mcp_manager {
            mcp.shutdown();
        }
    }
}

impl TurnContext {
    fn build(
        runtime: &mut Runtime,
        tool_rounds: usize,
        reads_this_turn: &HashSet<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) -> Result<TurnContext, ()> {
        let last_user = runtime.conversation.last_user_content();
        // Correction rounds are injected by the runtime after a cargo check failure.
        // They must be excluded from intent classification (no retrieval/mutation detection)
        // but must allow mutation so the model's corrective edit can go through the approval gate.
        let is_correction_round = last_user.is_some_and(|c| c.starts_with("[runtime:correction]"));
        let original_user_prompt = last_user.filter(|c| {
            !c.starts_with("=== tool_result:")
                && !c.starts_with("=== tool_error:")
                && !c.starts_with("[runtime:correction]")
        });
        let retrieval_intent = original_user_prompt
            .map(classify_retrieval_intent)
            .unwrap_or(RetrievalIntent::None);
        let requested_read_path: Option<String> = match &retrieval_intent {
            RetrievalIntent::DirectRead { path, .. } => Some(path.clone()),
            _ => None,
        };
        let direct_read_mode = match &retrieval_intent {
            RetrievalIntent::DirectRead { mode, .. } => Some(*mode),
            _ => None,
        };
        let investigation_required = original_user_prompt
            .map(|prompt| {
                requested_read_path.is_none()
                    && !user_requested_mutation(prompt)
                    && prompt_requires_investigation(prompt)
            })
            .unwrap_or(false);
        let mutation_allowed = is_correction_round
            || original_user_prompt
                .map(|p| user_requested_mutation(p) || user_requested_execution(p))
                .unwrap_or(false);
        let simple_edit_request = original_user_prompt.and_then(requested_simple_edit);
        let tool_surface = original_user_prompt
            .map(|p| {
                select_tool_surface(
                    p,
                    investigation_required,
                    mutation_allowed,
                    requested_read_path.is_some() || !reads_this_turn.is_empty(),
                )
            })
            .unwrap_or(if is_correction_round {
                // Correction rounds must use MutationEnabled so edit_file is available.
                ToolSurface::MutationEnabled
            } else if reads_this_turn.is_empty() {
                ToolSurface::AnswerOnly
            } else {
                ToolSurface::RetrievalFirst
            });
        let investigation_mode = original_user_prompt
            .map(detect_investigation_mode)
            .unwrap_or(InvestigationMode::General);
        let explicit_investigation_path_scope: Option<String> = if investigation_required {
            original_user_prompt.and_then(extract_investigation_path_scope)
        } else {
            None
        };
        let same_scope_reference = investigation_required
            && explicit_investigation_path_scope.is_none()
            && original_user_prompt.is_some_and(has_same_scope_reference);
        let investigation_path_scope: Option<String> =
            if let Some(scope) = explicit_investigation_path_scope {
                Some(scope)
            } else if same_scope_reference {
                trace_runtime_decision(
                    on_event,
                    "anchor_prompt_matched",
                    &[("kind", "same_scope".into())],
                );
                match runtime
                    .anchors
                    .last_scoped_search_scope()
                    .map(str::to_string)
                {
                    Some(scope) => {
                        trace_runtime_decision(
                            on_event,
                            "anchor_resolved",
                            &[("kind", "same_scope".into()), ("scope", scope.clone())],
                        );
                        Some(scope)
                    }
                    None => {
                        trace_runtime_decision(
                            on_event,
                            "anchor_missing",
                            &[("kind", "same_scope".into())],
                        );
                        runtime.finish_with_runtime_answer(
                            NO_LAST_SCOPED_SEARCH_AVAILABLE,
                            AnswerSource::RuntimeTerminal {
                                reason: RuntimeTerminalReason::InsufficientEvidence,
                                rounds: tool_rounds,
                            },
                            on_event,
                        );
                        return Err(());
                    }
                }
            } else {
                None
            };
        trace_runtime_decision(
            on_event,
            "investigation_mode_detected",
            &[
                ("mode", investigation_mode.as_str().into()),
                ("required", investigation_required.to_string()),
            ],
        );
        trace_runtime_decision(
            on_event,
            "investigation_path_scope",
            &[(
                "scope",
                investigation_path_scope
                    .as_deref()
                    .unwrap_or("none")
                    .to_string(),
            )],
        );
        trace_runtime_decision(
            on_event,
            "tool_surface_selected",
            &[("surface", tool_surface.as_str().into())],
        );
        let shell_request = original_user_prompt.and_then(requested_shell_command);
        if !investigation_required && tool_surface != ToolSurface::GitReadOnly {
            if let Some(cmd) = shell_request.as_ref() {
                if !is_permitted_shell_command(cmd) {
                    let first = cmd.split_whitespace().next().unwrap_or(cmd);
                    trace_runtime_decision(
                        on_event,
                        "shell_command_rejected",
                        &[
                            ("cmd", first.to_string()),
                            ("surface", tool_surface.as_str().to_string()),
                        ],
                    );
                    on_event(RuntimeEvent::Failed {
                        message: format!(
                            "shell command '{}' is not permitted. Allowed: cargo",
                            first
                        ),
                    });
                    return Err(());
                }
            }
        }
        Ok(TurnContext {
            retrieval_intent,
            requested_read_path,
            direct_read_mode,
            investigation_required,
            mutation_allowed,
            simple_edit_request,
            tool_surface,
            investigation_mode,
            investigation_path_scope,
            shell_request,
        })
    }
}

fn seed_pending_runtime_call(ctx: &TurnContext, state: &mut TurnState) {
    state
        .investigation
        .configure_usage_evidence_policy(usage_lookup_is_broad(
            ctx.investigation_mode,
            ctx.requested_read_path.as_deref(),
            ctx.investigation_path_scope.as_deref(),
        ));
    if !ctx.investigation_required && ctx.tool_surface != ToolSurface::GitReadOnly {
        if let Some(cmd) = ctx.shell_request.as_ref() {
            state.pending_runtime_call = Some(PendingRuntimeCall {
                input: ToolInput::Shell {
                    command: cmd.clone(),
                },
                seeded_pre_generation: true,
            });
        } else if let Some(edit) = ctx.simple_edit_request.as_ref() {
            state.pending_runtime_call = Some(PendingRuntimeCall {
                input: ToolInput::EditFile {
                    path: edit.path.clone(),
                    search: edit.search.clone(),
                    replace: edit.replace.clone(),
                },
                seeded_pre_generation: true,
            });
        } else {
            match &ctx.retrieval_intent {
                RetrievalIntent::DirectRead { path, .. } => {
                    state.pending_runtime_call = Some(PendingRuntimeCall {
                        input: ToolInput::ReadFile { path: path.clone() },
                        seeded_pre_generation: true,
                    });
                }
                RetrievalIntent::DirectoryListing { path } => {
                    state.pending_runtime_call = Some(PendingRuntimeCall {
                        input: ToolInput::ListDir { path: path.clone() },
                        seeded_pre_generation: true,
                    });
                }
                RetrievalIntent::None => {}
            }
        }
    }
}

/// Extracts the absolute file path from an edit_file or write_file pending payload.
/// Both tools use a null-byte-separated format:
///   v2: "v2\x00<abs_path>\x00..."
///   legacy: "<abs_path>\x00..."
fn extract_absolute_path_from_payload(payload: &str) -> Option<String> {
    const SEP: char = '\x00';
    let mut parts = payload.splitn(3, SEP);
    let first = parts.next()?;
    if first == "v2" {
        let abs = parts.next()?;
        if !abs.is_empty() {
            return Some(abs.to_string());
        }
        return None;
    }
    // Legacy: first segment is the absolute path.
    if std::path::Path::new(first).is_absolute() {
        return Some(first.to_string());
    }
    None
}

/// Returns true when the most recent user message in the conversation is an edit_file
/// tool error injected by the runtime. Used to detect the edit-repair failure pattern:
/// model emits garbled edit syntax after a failed edit, producing zero parsed tool calls.
pub(super) fn last_injected_was_edit_error(conversation: &Conversation) -> bool {
    conversation
        .last_user_content()
        .map(|c| c.starts_with("=== tool_error: edit_file ==="))
        .unwrap_or(false)
}

#[cfg(test)]
mod engine_unit_tests {
    use super::capture_session_head;
    use std::process::Command;
    use std::process::Stdio;
    use tempfile::TempDir;

    fn git(root: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} must succeed");
    }

    #[test]
    fn capture_session_head_on_empty_repo_returns_none() {
        let tmp = TempDir::new().unwrap();
        git(tmp.path(), &["init"]);
        // No commits — HEAD does not point to a valid object.
        assert!(capture_session_head(tmp.path()).is_none());
    }

    #[test]
    fn capture_session_head_with_commit_returns_hash() {
        let tmp = TempDir::new().unwrap();
        git(tmp.path(), &["init"]);
        git(
            tmp.path(),
            &[
                "-c",
                "user.email=thunk@example.invalid",
                "-c",
                "user.name=thunk",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        let hash = capture_session_head(tmp.path()).expect("must return a hash after commit");
        assert_eq!(hash.len(), 40, "SHA-1 must be 40 hex chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be hex"
        );
    }
}
