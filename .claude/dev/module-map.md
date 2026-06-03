# Module Map

Dependency order (bottom → top): `core/` → `storage/` / `tools/` → `runtime/` → `app/` → `tui/`

## src/core/
Owns `AppError`, `Result`, `Config` and all sub-configs (`LlmConfig`, `ProjectConfig`, `LspConfig`, `PromptPhysicsSettings`, provider configs, `CustomCommandDef`, etc.), and `load()`.
Also the known exception: `error.rs` imports `ToolError` from `tools/` for the `From<ToolError>` impl — tracked as tech debt.
Key files: `src/core/config.rs`, `src/core/error.rs`, `src/core/mod.rs`

## src/tools/
Owns concrete filesystem and Git actions, registration, approval contracts, and the `PendingAction` / `PendingTransaction` / `PendingApprovalStage` / `RiskLevel` types.
Must not parse assistant text, own conversation mutations, or decide investigation correctness.
`default_registry()` registers only `read_file` and `list_dir`.
`ToolRegistry::with_project_root()` adds `search_code`, `git_status`, `git_diff`, `git_log`, `git_branch`, `edit_file`, `write_file`, `shell`.
Key files: `src/tools/mod.rs`, `src/tools/registry.rs`, `src/tools/types.rs`, `src/tools/*.rs`

## src/runtime/lsp/
Owns the LSP server lifecycle, JSON-RPC transport, and definition/hover queries.
`LspManager` is the only public type; it starts rust-analyzer lazily on first query when `[lsp].enabled = true`.
`LspManager` is owned by `Runtime` — not registered in `ToolRegistry`.
Key files: `src/runtime/lsp/manager.rs`, `src/runtime/lsp/session.rs`, `src/runtime/lsp/transport.rs`, `src/runtime/lsp/protocol.rs`, `src/runtime/lsp/types.rs`

## src/runtime/index/
Owns project symbol and import extraction for the persistent index.
The extractor feeds `SymbolStore`; it does not own SQLite access or runtime dispatch policy.
Key files: `src/runtime/index/extractor.rs`, `src/runtime/index/types.rs`, `src/runtime/index/mod.rs`

## src/runtime/investigation/
Owns turn classification, investigation state, evidence gates, candidate selection, anchor state, and `InvestigationGraph`.
`InvestigationGraph` (petgraph) records import and definition edges; `promoted_candidates()` is advisory.
Key files: `src/runtime/investigation/investigation.rs`, `src/runtime/investigation/graph.rs`, `src/runtime/investigation/anchors.rs`, `src/runtime/investigation/tool_surface.rs`, `src/runtime/investigation/prompt_analysis.rs`, `src/runtime/investigation/search_query.rs`

## src/runtime/orchestration/
Owns request dispatch, the turn loop, tool round execution, generation, and context management.
Split across multiple files — no file owns more than one concern.
Key files:
- `engine.rs` — `Runtime::handle()`, submit/approve/reject dispatch, turn loop, `execute_and_handle()`, `execute_transaction()`, verification/correction loop
- `tool_round.rs` — `run_tool_round()`, search budget, non-candidate enforcement, LSP intercept, transaction collection
- `generation.rs` — `run_generate_turn()`, snapshot hint injection, prompt-physics refresh/recency injection
- `command_handlers.rs` — `CommandTool` allowlist for slash-command dispatch
- `turn_state.rs` — `TurnContext`, `TurnState`, `AnswerPhaseKind`, `PendingRuntimeCall`
- `engine_guards.rs` — `usage_lookup_is_broad()`, `extract_claimed_paths()`
- `context_policy.rs` — `ContextPolicy` derived from `BackendCapabilities.context_window_tokens`
- `context_cap.rs` — `cap_tool_result_blocks()`, `estimate_generation_prompt_chars()`
- `anchor_resolution.rs` — `run_last_read_file_anchor()`, `run_last_search_anchor()`
- `telemetry.rs` — `TurnPerformance`, context usage telemetry, `GenerationRoundLabel/Cause`

## src/runtime/protocol/
Owns the wire protocol between model text and typed tool inputs/results.
`tool_codec/` is a module (not a single file): `tool_parser.rs`, `tool_renderer.rs`, `tool_detector.rs`.
Must not dispatch tools, resolve paths, enforce surfaces, or decide answer admissibility.
Key files: `src/runtime/protocol/tool_codec/mod.rs`, `src/runtime/protocol/prompt.rs`, `src/runtime/protocol/prompt_physics.rs`, `src/runtime/protocol/response_text.rs`

## src/runtime/project/
Owns path confinement types: `ProjectRoot`, `ProjectPath`, `ProjectScope`, `ResolvedToolInput`, `resolve()`.
`tools/` imports from here (intentional bidirectional dependency — tracked in architecture.md).
Key files: `src/runtime/project/resolver.rs`, `src/runtime/project/resolved_input.rs`, `src/runtime/project/project_root.rs`, `src/runtime/project/project_path.rs`, `src/runtime/project/project_snapshot.rs`

## src/llm/
Owns the backend abstraction and all provider implementations (`mock`, `llama_cpp`, `openai`, `ollama`, `openrouter`, `groq`).
Must not decide terminals, enforce tool permissions, or judge evidence.
Interacts with `runtime/` only through `GenerateRequest`, `BackendEvent`, and `BackendCapabilities`.
Key files: `src/llm/backend.rs`, `src/llm/providers/mod.rs`, `src/llm/providers/*.rs`

## src/storage/
Owns SQLite schema (v5), CRUD for saved sessions, and persistent symbol/import index storage.
Schema: `sessions` table with `project_root`, `last_read_file`, `last_search_query`, `last_search_scope`; `session_messages` table keyed by `(session_id, seq)`; `index_symbols`, `index_imports`, and `file_metadata` tables for the persistent index.
Must not know the system prompt, runtime correction policy, or tool semantics.
Key files: `src/storage/session/store.rs`, `src/storage/session/schema.rs`, `src/storage/session/types.rs`, `src/storage/index/store.rs`, `src/storage/index/types.rs`

## src/app/
Owns bootstrap, config loading, path discovery, backend construction, tool-registry construction, session restore, autosave, event logging.
`AppContext` wraps `Runtime` + `ActiveSession` + optional `SessionLog`; TUI works through `AppContext::handle()`.
`ActiveSession` (`app/session.rs`) is the only layer that converts between runtime `Message` and stored records.
Bootstrap reads optional project `.thunk/THUNK.md` (falling back to `THUNK.md` at project root) and passes it into runtime prompt physics config; it does not persist `THUNK.md` content into sessions.
Must not implement runtime policy or parse tool syntax.
Key files: `src/app/mod.rs`, `src/app/context.rs`, `src/app/session.rs`, `src/app/paths.rs`, `src/app/config.rs`

## src/tui/
Owns command parsing (`tui/commands/mod.rs`), input handling, screen rendering, and `RuntimeEvent` → UI state mapping.
No business logic. No tool dispatch. No direct runtime calls except via `RuntimeRequest`.
Key files:
- `src/tui/mod.rs` — terminal setup/teardown and module declarations
- `src/tui/app.rs` — TUI event loop, render scheduling, worker reply handling
- `src/tui/worker.rs` — background `AppContext` command runner
- `src/tui/cursor.rs` — cursor shape and terminal affordance sync
- `src/tui/keybindings.rs` — key event dispatch
- `src/tui/events.rs` — `RuntimeEvent` to `AppState` mutations
- `src/tui/format.rs` — UI formatting and command-output summarization helpers
- `src/tui/state.rs` — mutable UI state
- `src/tui/input.rs` — input editing, history, reverse search, launcher, autocomplete
- `src/tui/collapsible.rs` — pure collapsible summary classification; no renderer dependency
- `src/tui/commands/mod.rs` — slash command parser, autocomplete names, launcher entries
- `src/tui/commands/dispatch.rs` — command dispatch to worker/runtime requests
- `src/tui/renderer/mod.rs` — renderer, transcript painting, overlays, approval widget, spinner
- `src/tui/renderer/buffer.rs` — cell buffer
- `src/tui/renderer/diff.rs` — frame diff writer
- `src/tui/renderer/style.rs` — `Theme`, colors, packed style
- `src/tui/renderer/symbols.rs` — symbol pool

Renderer exception: `Renderer::render()` takes `&mut AppState` because `paint_transcript()` has load-bearing render side effects documented in `renderer/mod.rs`: it updates `state.max_scroll`, consumes `state.scroll_to_message_idx`, adjusts `state.scroll_offset`, and repopulates `state.visible_collapsible_ids` so collapsible viewport focus works.
`src/tui/renderer/transcript.rs` does not exist in the current tree; transcript rendering lives in `renderer/mod.rs`.

## src/logging/
Owns `SessionLog`: per-session append-only log file opened in `data/logs/`.
Advisory only — failures are silently ignored. Not part of runtime control flow.
Key file: `src/logging/mod.rs`
