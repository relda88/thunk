# debug-runtime

Activate when diagnosing runtime failures, protocol parse errors, tool
dispatch problems, mutation issues, or session/restore problems.

## Reference Materials

**Project learnings** (review before diagnosing — known failure patterns and gotchas):
!cat .claude/skills/debug-runtime/learnings.md

---

## When to use this skill
- Tools are failing at 0ms with no visible error
- Protocol parse failures — model emitting malformed tool syntax
- Mutation approval flow is broken
- Session restore is not working correctly
- Trace events are missing or unexpected

## Step 1 — Enable tracing

```bash
THUNK_TRACE_RUNTIME=1 cargo run --release
```

Trace events format: `[runtime:trace] event=<name> key=value ...`
Perf events format: `[runtime:perf] rounds=N tool_ms=N total_turn_ms=N`

`tool_ms=0` or `tool_ms=1` across multiple tools = tools not executing,
failure happening before dispatch. Check resolver or surface enforcement.

## Step 2 — Match symptom to entry point

**Tool fails at 0ms:**
- Start: `run_tool_round()` in `src/runtime/orchestration/tool_round.rs`
- Check: surface enforcement, resolver path confinement, scope injection
- Scope path is a file not a directory? → `resolve_scope()` in `resolver.rs`

**Protocol parse failure:**
- Start: `src/runtime/protocol/tool_codec/tool_parser.rs`
- Then: malformed/fabricated/garbled branches in `run_turns_with_initial_reads()`
- These branches decide: correct once or terminate

**Search/read/surface enforcement:**
- Start: `run_tool_round()` — owns scope injection, surface checks,
  weak-query rejection, list-before-search, search budget, duplicate reads,
  non-candidate reads, read caps, cycle detection

**Wrong candidate or wrong answer admitted:**
- Start: `InvestigationState::record_search_results()` — candidate classification
- Then: `record_read_result()` — evidence acceptance
- Then: `best_candidate_for_mode()` — candidate selection
- Then: answer-guard branches in `run_turns_with_initial_reads()`

**Mutation problems:**
- Full path: `resolve()` → tool `run()` → `PendingAction` → `PendingApprovalStage` / `PendingTransaction` → `execute_approved()` → `handle_approve()`
- Path rejection: `resolver.rs`
- Proposal validation: the tool itself
- Approval branching, LSP pre-check, verify/correction, and transaction rollback: `engine.rs`

**Session/restore problems:**
- Session store: `src/storage/session/store.rs`
- Restore logic: `src/app/session.rs`
- System prompt is never persisted — always rebuilt from config on restore

## Step 3 — Test entry points by failure type

| Failure | Test file |
|---------|-----------|
| Retrieval and scope | `src/runtime/tests/investigation.rs`, `src/runtime/tests/path_scope.rs` |
| Approval flow | `src/runtime/tests/approval.rs` |
| Answer finalization | `src/runtime/tests/finalization.rs` |
| Protocol failures | `src/runtime/tests/tool_round.rs` |
| Integration/filesystem | `src/runtime/tests/integration.rs` |

## Step 4 — Common false alarms

- Trace exists in logs but not on screen → expected, `AppContext` does not
  forward `RuntimeTrace` events to TUI
- `tool_ms=0` on first session run → rust-analyzer cold start (30s timeout)
- 21+ second LSP call → rust-analyzer indexing, not a bug, warm on next call
- `lsp_definition: no definition found` → coordinates landed on comment line,
  check `is_declaration_line()` in `tool_round.rs`
