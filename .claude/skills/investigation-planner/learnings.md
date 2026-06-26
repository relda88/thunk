# Learnings — investigation-planner

Project-specific observations that improve evidence-first investigation in thunk (Rust CLI, runtime-owned control flow, slice discipline, Phase 43+).

## Format
Each entry: **Date | Phase | Subsystem** / Observation / Evidence / Action-Rule / Impact (High/Med/Low)

## Graduation criteria
An entry graduates to `rules/invariants.md` (## Evolved Invariants) when: validated across 2+ phases, systemic (affects multiple files or a core invariant), and actionable as a hard rule. Graduated entries are tagged [GRADUATED] and archived here.

---

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
