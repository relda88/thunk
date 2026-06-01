# Enforced Invariants

## Mutation Approval Gate
`ShellTool`, `EditFileTool`, `WriteFileTool` always return `ToolRunResult::Approval(PendingAction)`.
The only materialization path is `ToolRegistry::execute_approved()` in `src/tools/registry.rs`.
There is no bypass. Never add one.

## Shell Allowlist
`is_permitted_shell_command()` at `src/runtime/investigation/prompt_analysis.rs` — matches only `"cargo"`.
Enforced in `TurnContext` construction in `engine.rs` (~line 1535): non-permitted commands suppress shell seeding.
Shell seeding is suppressed entirely on `GitReadOnly` turns.

## Surface Enforcement
`tool_allowed_for_surface()` at `src/runtime/investigation/tool_surface.rs`.
Surfaces and tool sets defined in `TOOL_SURFACE_DEFINITIONS` (static registry).
`RetrievalFirst` includes `lsp_definition`. `GitReadOnly` includes `git_branch`.
Mutation tools (`edit_file`, `write_file`, `shell`) return `None` from `SurfaceTool::from_input()` — they bypass surface enforcement and go through the approval path only.

## Evidence Gates
Eight named gates (plus sub-gates 5.5, 6a) in `InvestigationState::record_read_result()` in `investigation.rs`.
`evidence_ready()` at `investigation.rs:617` — requires `search_produced_results && useful_accepted_candidate_reads >= useful_candidate_reads_target`.
Gates are never weakened. Never add a bypass.

## System Prompt
Always built fresh via `build_system_prompt()` from config — never persisted to SQLite.
Always called with `include_mutation_tools: false` (`engine.rs:105`).
Mutation tools appear only in the ephemeral per-turn hint for `MutationEnabled` turns.

## Session Scoping
All tool inputs confined via `resolve()` in `src/runtime/project/resolver.rs`.
`ProjectRoot::new()` canonicalizes and validates at construction; on Windows, strips the `\\?\` UNC prefix after `fs::canonicalize`.

## LSP Is Never Load-Bearing
`LspManager` errors produce an empty `LspDefinitionOutput`, not a terminal answer.
The runtime must not depend on LSP availability for correctness. LSP results update `InvestigationGraph` only; graph candidates are advisory fallbacks, not primary candidates.
`LspManager` is dispatched in `tool_round.rs` before `registry.dispatch()` because it requires `&mut self`; it is not registered in `ToolRegistry`.

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
