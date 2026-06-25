# Learnings — investigation-planner

Project-specific observations that improve evidence-first investigation in thunk (Rust CLI, runtime-owned control flow, slice discipline, Phase 43+).

## Format
Each entry: **Date | Phase | Subsystem** / Observation / Evidence / Action-Rule / Impact (High/Med/Low)

## Graduation criteria
An entry graduates to `rules/invariants.md` (## Evolved Invariants) when: validated across 2+ phases, systemic (affects multiple files or a core invariant), and actionable as a hard rule. Graduated entries are tagged [GRADUATED] and archived here.

---

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
