# Enforced Invariants

## Mutation Approval Gate
`ShellTool`, `EditFileTool`, `WriteFileTool` always return `ToolRunResult::Approval(PendingAction)`.
The only materialization path is `ToolRegistry::execute_approved()` at `src/tools/registry.rs:66`.
Single approvals and grouped approvals are wrapped in `PendingApprovalStage` / `PendingTransaction` at `src/tools/pending.rs`.
There is no bypass. Never add one.

## Shell Allowlist
`is_permitted_shell_command()` at `src/runtime/investigation/prompt_analysis.rs:269` — matches only `"cargo"`.
Enforced in `TurnContext` construction in `engine.rs`: non-permitted commands suppress shell seeding.
Shell seeding is suppressed entirely on `GitReadOnly` turns.

## Surface Enforcement
`tool_allowed_for_surface()` at `src/runtime/investigation/tool_surface.rs:260`.
Surfaces and tool sets defined in `TOOL_SURFACE_DEFINITIONS` (static registry of built-in tools).
`RetrievalFirst` includes `lsp_definition`. `GitReadOnly` includes `git_branch`.
Mutation tools (`edit_file`, `write_file`, `shell`) return `None` from `SurfaceTool::from_input()` — they bypass surface enforcement and go through the approval path only.
Dynamic (MCP) tools are not in the static surface registry: `tool_allowed_for_surface()` takes a runtime-held `dynamic_allowed: &HashSet<String>` and admits a `ToolInput::DynamicTool` only when its name is whitelisted for the current surface. `DynamicTool` also returns `None` from `SurfaceTool::from_input()`.

## Evidence Gates
Eight named gates (plus sub-gates 5.5, 6a) in `InvestigationState::record_read_result()` in `investigation.rs`.
`evidence_ready()` at `src/runtime/investigation/investigation.rs:552` — requires `search_produced_results && useful_accepted_candidate_reads >= useful_candidate_reads_target`.
Gates are never weakened. Never add a bypass.

## System Prompt
Always built fresh via `build_system_prompt()` from config — never persisted to SQLite.
Always called with `include_mutation_tools: false` (positional arg in the `build_system_prompt()` call at `src/runtime/orchestration/engine.rs:231`).
Mutation tools appear only in the ephemeral per-turn hint for `MutationEnabled` turns.

## Prompt Physics
Prompt physics is enabled by default via `[prompt_physics].enabled`.
`.thunk/THUNK.md` is read during app bootstrap and passed to `PromptPhysicsConfig` as an optional primacy anchor. Falls back to `THUNK.md` at project root for backward compatibility.
Periodic refresh and recency-field messages are appended per generation in `src/runtime/orchestration/generation.rs`; they are request-local and must not be persisted as conversation history.

## Verification and Correction
`project.verify_command` is a language-agnostic runtime verification command run after approved `edit_file` / `write_file` mutations.
`project.max_correction_attempts` bounds self-correction attempts after verify failure; `correction_attempts` is runtime state and must reset on success or terminal failure.
Transactions run `verify_command` after all edits, but intentionally skip the self-correction loop.

## Session Scoping
All tool inputs confined via `resolve()` in `src/runtime/project/resolver.rs`.
`ProjectRoot::new()` canonicalizes and validates at construction; on Windows, strips the `\\?\` UNC prefix after `fs::canonicalize`.

## LSP Is Never Load-Bearing
`LspManager` errors produce an empty `LspDefinitionOutput`, not a terminal answer.
The runtime must not depend on LSP availability for correctness. LSP results update `InvestigationGraph` only; graph candidates are advisory fallbacks, not primary candidates.
`LspManager` is dispatched in `tool_round.rs` before `registry.dispatch()` because it requires `&mut self`; it is not registered in `ToolRegistry`.
Pre-edit and post-edit diagnostic checks are skipped for files whose extension is not in `LspConfig.extensions`.

## InvestigationGraph Is Advisory
`InvestigationGraph` (petgraph) owned by `InvestigationState.graph` records import edges and LSP definition edges.
`promoted_candidates()` is consulted as a fallback read candidate; it does not override the search-candidate set or evidence gates.

## TUI Render State Exceptions
`Renderer::render()` intentionally takes `&mut AppState`.
`paint_transcript()` intentionally mutates `state.max_scroll` and `state.visible_collapsible_ids`, consumes `state.scroll_to_message_idx`, and may adjust `state.scroll_offset`.
This is a justified exception: the mutation is load-bearing for collapsible viewport focus and is documented in `src/tui/renderer/mod.rs`.

## TUI Spinner
`spin_tick` increments only when `state.is_busy`.
The zero-cells render test depends on this: an unchanged non-busy state must render with zero changed cells.

## Terminal Key Protocol
`Alt+[` is terminal-limited on macOS/crossterm: `ESC [` is interpreted as a CSI prefix.
Without kitty keyboard protocol support, the `Alt+[` binding never fires even though `keybindings.rs` contains it.

## Evolved Invariants

Invariants in this section originated as project learnings (discovered during implementation) and were graduated after validation across multiple phases. Each entry references its source learning.

<!-- Entries will be added here via /wrap-up as learnings graduate. -->
