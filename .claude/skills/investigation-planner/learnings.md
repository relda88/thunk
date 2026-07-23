# Learnings — investigation-planner

Project-specific observations that improve evidence-first investigation in thunk (Rust CLI, runtime-owned control flow, slice discipline, Phase 43+).

## Format
Each entry: **Date | Phase | Subsystem** / Observation / Evidence / Action-Rule / Impact (High/Med/Low)

## Graduation criteria
An entry graduates to `rules/invariants.md` (## Evolved Invariants) when: validated across 2+ phases, systemic (affects multiple files or a core invariant), and actionable as a hard rule. Graduated entries are tagged [GRADUATED] and archived here.

---

**2026-06-30 | Phase 49 | gui/dto**
**Observation**: The GUI is a pure projection of the same worker channel the TUI uses — not a parallel implementation. `RuntimeEventDto::try_from` returns `Err(())` for advisory variants (`BackendTiming`, `BackendTokenCounts`, `RuntimeTrace`, `PromptAssembled`) so they never reach the webview, and `worker_reply_to_dto` is the single `WorkerReply`→event map. Adding a `RuntimeEvent` variant without a DTO arm is a compile error (good), but a wrong classification silently leaks or drops UI events.
**Evidence**: `src/app/dto.rs` (`RuntimeEventDto`, `worker_reply_to_dto`); commit 29a7abe; mirrors `src/tui/events.rs`.
**Action/Rule**: When adding a `RuntimeEvent`/`WorkerReply` variant, update BOTH frontend projections (`tui/events.rs` and `app/dto.rs`) and decide advisory-vs-surfaced explicitly. Keep all business logic upstream of the DTO; the DTO carries no policy.
**Impact**: Med

**2026-07-02 | Phase 49 | gui/dto**
**Observation**: Twice within slice 49.9, a distinct backend outcome was folded into a generic carrier instead of getting its own variant — `WorkerReply::ResetOk` originally mapped to `RuntimeEventDto::SystemMessage{text: "Session cleared."}`, and `/help` was dispatched through `run_command`'s `Err(String)` channel as a formatted text blob. Both were fixed by adding a dedicated discriminator instead.
**Evidence**: commit 1ff210c; `src/app/dto.rs:232` (`ResetOk` variant + test `reset_ok_maps_to_reset_ok_dto_and_serializes`); `src/gui/commands.rs:23` (`get_help_commands`), comment at line 137 noting the frontend now intercepts `/help` directly.
**Action/Rule**: When a backend outcome needs frontend-specific rendering (a toast, a structured list, a distinct UI state), give it its own DTO variant or command — never route it through `SystemMessage{text}` or a stringly-typed `Err()` as an implicit discriminator. Both occurrences so far are within the same slice (49.9) — this has not yet cleared the 2-phase graduation bar; revisit if a third instance appears in a later phase.
**Impact**: High

**2026-07-22 | Phase 50 | verify/worker**
**Observation**: A third occurrence of the generic-string-carrier-as-implicit-discriminator pattern, and the first in a phase other than 49 — the ok/unavailable/failed distinction for `project.verify_command` results is encoded ONLY in the text content of `RuntimeEvent::SystemMessage(String)` (sync path) and `WorkerReply::DeferredVerification(String)` (the actual live path, since `deferred_verify` defaults to `true`). There is no structural success/failure boolean anywhere downstream — not in the `WorkerReply` variant, not in `RuntimeEventDto::SystemMessage { text }` on the GUI side, not in the frontend `SystemMessage` component (which just renders `text` verbatim). Slice 50.7 fixed the underlying exit-status-blindness bug but deliberately left this carrier shape alone — a dedicated structured variant would be a larger redesign than that slice's scope.
**Evidence**: `src/tui/worker.rs` `WorkerReply::DeferredVerification(String)` (built in two inlined closures, `WorkerCmd::Handle` and `WorkerCmd::RebuildFile`); `src/runtime/types.rs:428` `RuntimeEvent::SystemMessage(String)`; `src/app/dto.rs:337-339` `worker_reply_to_dto` mapping `DeferredVerification(msg)` to `RuntimeEventDto::SystemMessage { text: msg }`; `frontend/src/components/SystemMessage.tsx:17-40`.
**Action/Rule**: This is the third confirmed instance of the pattern first logged in the entry above (Phase 49, gui/dto), and the second separate phase to exhibit it (Phase 49 and Phase 50) — per this file's own graduation criteria ("validated across 2+ phases, systemic, actionable as a hard rule"), this now meets the stated bar for promotion to `.claude/rules/invariants.md` (## Evolved Invariants). Flagging for explicit review/confirmation rather than graduating it unilaterally — do not silently add it without a decision.
**Impact**: High — flagged for graduation review, not yet promoted.

**2026-06-30 | Phase 49 | app/backend**
**Observation**: Both frontends share one bootstrap: `spawn_backend` returns a `BackendHandle { cmd_tx, reply_rx }` wiring the worker + fs-watcher + proactive-scan threads, and `src/gui/commands.rs::run_command` delegates straight to `crate::tui::commands::{parse, resolve_command, resolve_custom_command}`. Slash-command semantics and backend threading are single-sourced, not duplicated in `gui/`.
**Evidence**: `src/app/backend.rs` `spawn_backend`; `src/gui/commands.rs`; commits 0d32298, 09677c5.
**Action/Rule**: A new frontend must consume `spawn_backend` + `tui::commands`, never re-implement command parsing or thread wiring. If either needs frontend-specific behavior, push the split into the shared layer, not into `gui/`.
**Impact**: Med

**2026-06-29 | Phase 47 | shell-tier**
**Observation**: The runtime spawns shell commands directly (no interpreter), so any shell metacharacter (`|` `>` `<` `;` `$` `` ` `` `&` newline) cannot execute and MUST route to Tier-3 (`bash -c`, exec-gated). `classify_shell_tier` checks metachars BEFORE program lookup — `ls | grep` is Exec, not ReadOnly, despite `ls` being a Tier-1 program.
**Evidence**: `src/runtime/investigation/shell_tier.rs` `has_shell_metachar` (checked before `base_tier`); commit 893a018.
**Action/Rule**: When classifying or routing a shell command, metachar detection precedes program classification. Never assume a Tier-1 program name makes the whole command read-only — a pipe or redirect overrides it.
**Impact**: High

**2026-06-26 | Phase 46 | generation/mcp**
**Observation**: MCP tool names must be threaded into THREE independent per-turn surfaces, not one: the tool-surface hint (`run_generate_turn` → `dynamic_tool_names`), the recency field (`recency_field_message`), and the telemetry estimator (`estimate_generation_prompt_chars`). Missing any single one yields "model can't call the MCP tool" or a silent telemetry undercount.
**Evidence**: commits bd689a7 (hint), 0899f5d (recency + estimator); `src/runtime/orchestration/generation.rs`, `context_cap.rs`, `prompt_physics.rs:71`.
**Action/Rule**: When adding any new runtime-owned tool name, grep for every consumer of `allowed_tool_names()`/`mutation_tool_names()` and thread the dynamic set into each. There is no single chokepoint.
**Impact**: High

**2026-06-26 | Phase 46 | memory**
**Observation**: The model emits `[REMEMBER: ... | category]` proposals that frequently contain task imperatives ("create a module…", "add logging…") rather than durable personal facts. Unfiltered, the memory store fills with task instructions.
**Evidence**: `src/runtime/protocol/memory_parser.rs` `is_imperative()` + `IMPERATIVE_MARKERS`; commit c08536d; tests `imperative_first_word_is_dropped`.
**Action/Rule**: Any model-proposed fact stream needs a first-word imperative filter before persistence. Filter at parse time in `parse_memory_proposals`, not at write time.
**Impact**: Med

**2026-06-26 | Phase 46 | resolver/mcp**
**Observation**: Path-confinement errors are an opportunity to redirect the model, not just reject it. `EscapesRoot` ToolError and `list_dir` description now point the model at `mcp::filesystem::list_directory` for out-of-project paths.
**Evidence**: `src/runtime/project/resolver.rs` `From<PathResolutionError>`, `src/tools/core/list_dir.rs` spec; commit c08536d.
**Action/Rule**: When a confinement guard rejects an action that an MCP tool could legitimately perform, embed the redirect in the error string. Keep error text and tool description in sync.
**Impact**: Med

**2026-06-26 | Phase 46 | memory/app-paths**
**Observation**: Memory must write correctly when launched from a bare directory with no `.git` ancestor. `AppPaths.project_label` is `Some(git_root.file_name())` or `None`; the `memory_write_scope` helper centralizes the scope decision so the 3 write sites don't each re-derive it.
**Evidence**: `src/app/paths.rs` `project_label`, `src/runtime/orchestration/memory_handlers.rs`; commit 6d454b8.
**Action/Rule**: Never assume a project root exists in memory/storage write paths. Route scope decisions through `memory_write_scope`; test the no-`.git` fallback explicitly.
**Impact**: Med

**2026-06-25 | Phase 45 | mcp**
**Observation**: MCP JSON-RPC responses are double-enveloped — `tools/list` results are at `result["result"]["tools"]`, not `result["tools"]`. `tools/call` response content is at `response["result"]["content"]`.
**Evidence**: `src/runtime/mcp/manager.rs` — bug fix commit after 45.3 shipped with wrong path; `discover_tools` silently returned empty.
**Action/Rule**: Always verify the full JSON-RPC envelope path in MCP investigations. Never assume unwrapped result — trace through `McpSession::call` → `recv_response` to see what's actually returned.
**Impact**: High

**2026-06-25 | Phase 45 | mcp**
**Observation**: MCP tools must bypass `execute_approved()` via an intercept — they are not registered in `ToolRegistry`, so registry dispatch returns `NotFound`. Pattern mirrors LSP intercept: `tool_name.starts_with("mcp::")` check in `execute_and_handle`, decodes payload, calls server, re-enters generation via `self.run_turns(0, on_event)`.
**Evidence**: `src/runtime/orchestration/engine.rs` MCP execution branch; `src/runtime/orchestration/tool_round.rs` MCP intercept (~line 888).
**Action/Rule**: Any new tool category that needs `&mut self` on Runtime (can't go through `registry.execute_approved`) follows the LSP/MCP intercept pattern. Check `registry.rs:execute_approved` signature first — if it lacks Runtime access, intercept is required.
**Impact**: High

**2026-06-25 | Phase 45 | mcp**
**Observation**: Read-only MCP tools must re-enter generation after result commit (`self.run_turns(0, on_event)`), not use the mutation terminal-answer path. Using terminal answer for a retrieval tool silently breaks synthesis.
**Evidence**: `src/runtime/orchestration/engine.rs` execute_and_handle MCP branch; investigation report item 4 design fork.
**Action/Rule**: In any approval-gated tool that returns a result for model synthesis (not a mutation): commit results then call `run_turns`, never `mutation_complete_final_answer`.
**Impact**: High

**2026-06-25 | Phase 45 | mcp**
**Observation**: `mcp_manager`'s `&mut` borrow must be scoped to a match arm so the owned result releases before `commit_tool_results`/`run_turns` re-borrow `self`. Borrow-checker constraint, not a logic issue.
**Evidence**: `src/runtime/orchestration/engine.rs` MCP execution branch comment.
**Action/Rule**: When implementing any new intercept path that calls `&mut self.some_manager` and then calls other `&mut self` methods, scope the manager borrow to a block and extract the result before proceeding.
**Impact**: Med

**2026-06-25 | Phase 45 | tool-surface**
**Observation**: `DynamicToolSpec` must be a separate struct from `ToolSpec` — MCP tool names are `String`, not `&'static str`. `ToolRegistry` was re-keyed from `HashMap<&'static str, _>` to `HashMap<String, _>` to accommodate dynamic registration.
**Evidence**: `src/tools/types.rs` DynamicToolSpec, `src/tools/registry.rs:20`.
**Action/Rule**: Any new tool category with runtime-owned names needs `DynamicToolSpec`, not `ToolSpec`. Never use `Box::leak()` to fake `&'static str` — re-key the registry instead.
**Impact**: Med

**2026-06-25 | Phase 44 | runtime**
**Observation**: Background verification must be deferred to a spawned thread and surfaced as a passive `DeferredVerification` reply — not inline in the mutation path. The existing drain loop (`app.rs:89-91`) tolerates late replies without new machinery.
**Evidence**: `src/tui/worker.rs` Handle arm verify spawn; `src/tui/app.rs:89-91`.
**Action/Rule**: Any result that should arrive after a turn completes (background check, async notification) uses `reply_tx.clone()` + `thread::spawn` + `WorkerReply::DeferredVerification`. No new channel needed — existing drain loop handles arbitrary-timing replies.
**Impact**: Med

**2026-06-25 | Phase 43 | orchestration**
**Observation**: `rel_path`/`single_rel_path` naming inconsistency across 43.5-43.8 call sites caused a near-miss silent dedup failure. Before/after symbol capture in the single-approval path used `single_rel_path` (threaded via Option) while after-capture used `rel_path` (re-borrowed from the Option) — same value, different names.
**Evidence**: `src/runtime/orchestration/engine.rs` single-approval path, Phase 43.5 post-implementation audit.
**Action/Rule**: In any path-normalization investigation, explicitly verify every normalization call site produces identical output (same strip_prefix base, same separator, same failure handling). Name consistency is not a guarantee of behavioral identity — verify the code.
**Impact**: High
