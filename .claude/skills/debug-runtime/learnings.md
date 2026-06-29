# Learnings — debug-runtime

Project-specific observations for diagnosing runtime and orchestration failures in thunk.

## Format
Each entry: **Date | Phase | Subsystem** / Observation / Evidence / Action-Rule / Impact (High/Med/Low)

## Graduation criteria
Same as investigation-planner/learnings.md. Graduated entries tagged [GRADUATED] and archived.

---

**2026-06-29 | Phase 47 | orchestration**
**Observation**: The exec-gate deny (Tier-3 shell with `exec_enabled=false`) must return `TerminalAnswer`, not `continue`. Failed tool calls don't update `last_call_key`, so a `continue` lets the model re-emit the identical blocked command → deny→retry spiral. The denial also needs a `SystemMessage` event because the model cannot self-correct it (only `/exec on` can), so the model-facing `accumulated` buffer alone is insufficient.
**Evidence**: `src/runtime/orchestration/tool_round.rs:916-940` exec-gate intercept; commits d31b9ef, ac855c7.
**Action/Rule**: Any runtime-level deny the model cannot fix by retrying must terminate the turn (TerminalAnswer) AND emit a SystemMessage to the TUI. Check whether the deny path updates `last_call_key` before choosing `continue`.
**Impact**: High

**2026-06-29 | Phase 47 | orchestration**
**Observation**: A seeded `shell_read` (Tier-1) command completing does NOT enter the PostRead answer phase automatically — `shell_read` is invisible to `reads_this_turn` (only `read_file` populates it), so synthesis never triggers. An explicit `state.answer_phase = PostRead` is required in the Completed branch, mirroring the DirectoryListing arm.
**Evidence**: `src/runtime/orchestration/engine.rs:1980` (`if ctx.shell_request.is_some()`); commit ac855c7.
**Action/Rule**: When adding any new immediate/read-only tool whose output should drive answer synthesis, confirm it feeds the answer-phase trigger. If it doesn't populate `reads_this_turn`, set `answer_phase` explicitly in its Completed arm.
**Impact**: Med

**2026-06-26 | Phase 46 | mcp**
**Observation**: Symptom "model emits a well-formed `mcp::server::tool` call but nothing executes" almost always means the tool name never reached the per-turn surface hint, so the surface policy rejected it before dispatch — not a transport or registration bug.
**Evidence**: commit bd689a7; `src/runtime/orchestration/generation.rs` `hint_tools` extension with `dynamic_tool_names`.
**Action/Rule**: For "MCP call is a no-op", first confirm the tool name appears in the rendered surface hint for that turn (check `dynamic_tool_names` is non-empty and threaded). Only then investigate transport/discovery.
**Impact**: High

**2026-06-25 | Phase 45 | mcp**
**Observation**: A silent empty result from `discover_tools` (no tools discovered, no error) is almost always the JSON-RPC envelope path bug — `result["tools"]` instead of `result["result"]["tools"]`. The session starts, servers connect, but zero tools appear in the prompt.
**Evidence**: `src/runtime/mcp/manager.rs:171` bug fix (65117c4).
**Action/Rule**: When debugging "MCP tools not appearing in prompt": first check the envelope path in `discover_tools`. Add a temporary `eprintln!("{:?}", result)` after the `call()` to inspect the full envelope before path traversal.
**Impact**: High

**2026-06-25 | Phase 44 | runtime**
**Observation**: `deferred_verify: bool` on Runtime defaults to `true` — synchronous verify (correction loop) is skipped by default. Tests that depend on the synchronous correction path must chain `.with_deferred_verify(false)` on the Runtime builder or they silently stop exercising the correction logic.
**Evidence**: `src/runtime/orchestration/engine.rs` deferred_verify field; `src/runtime/tests/approval.rs:703,798,871`, `src/runtime/tests/engine.rs:1944`.
**Action/Rule**: When debugging "correction loop not firing" or "verify command not running synchronously": check whether `deferred_verify` is true. In new tests that need synchronous verify, always add `.with_deferred_verify(false)`.
**Impact**: Med

**2026-06-25 | Phase 43 | orchestration**
**Observation**: `insert_step_after` in the sequence executor renumbers positions correctly under repeated insertions because `step.position` is always read fresh from `get_current_step` (which reflects all prior shifts). The key invariant: insert only at positions >= `current_idx`; never renumber positions < `current_idx`.
**Evidence**: `src/storage/tasks/edit_store.rs` insert_step_after; Phase 43.8 audit item 3.
**Action/Rule**: When debugging sequence executor position bugs: verify `current_idx` in `edit_sequences` table matches expected value. Any position < current_idx should never be shifted — that's the corruption signature.
**Impact**: Med
