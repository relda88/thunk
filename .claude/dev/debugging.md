# Debugging

## Environment

Set `THUNK_TRACE_RUNTIME=1` (any non-empty value) to enable runtime decision tracing. There is no `PARAMS_TRACE_RUNTIME` — that name does not exist. Code: `src/runtime/trace.rs`.

## Trace Formats

- Decision traces: `[runtime:trace] event=<name> key=value ...` emitted by `trace_runtime_decision()` in `src/runtime/trace.rs`. A local copy of the same helper exists in `src/runtime/investigation/investigation.rs` for investigation-local tracing.
- Performance traces: `[runtime:perf] ...` emitted by `TurnPerformance` in `src/runtime/orchestration/telemetry.rs`. Records round labels, causes, prompt sizes, backend timing totals, tool time, and total turn time, then emits a summary at turn end.
- `AppContext::handle()` logs `RuntimeTrace` and `BackendTiming` events and deliberately does not forward them to the TUI. If a trace line appears in logs but not on screen, that is expected. Code: `src/app/context.rs`.

## Protocol Parse Failures

Start with `src/runtime/protocol/tool_codec/tool_detector.rs` (fabricated-exchange detection, malformed-block detection) and `tool_parser.rs` (parse logic). Then inspect the malformed/fabricated/garbled branches in `run_turns_with_initial_reads()` in `engine.rs`. Those branches decide whether the response is corrected once or terminated.

## Search, Read, and Surface Enforcement

Start with `run_tool_round()` in `src/runtime/orchestration/tool_round.rs`. That function owns scope injection/clamping, surface checks, weak-query rejection, list-before-search blocking, search budget, duplicate reads, non-candidate reads, read caps, cycle detection, and dispatch-time terminals.

`lsp_definition` is intercepted in `run_tool_round()` before `registry.dispatch()`. Debugging LSP issues: check `LspManager::start()` (probe + spawn logic), `src/runtime/lsp/session.rs` (JSON-RPC session), and the `query_definition` call site in `tool_round.rs`.

## Wrong Candidate or Wrong Answer Admissions

Inspect `InvestigationState::record_search_results()`, `InvestigationState::record_read_result()`, `best_candidate_for_mode()`, and the answer-guard branches in `run_turns_with_initial_reads()`. Also check `InvestigationGraph::promoted_candidates()` — if graph edges are promoting unexpected candidates, the import extraction or `record_definition_target()` call may be the source. Code: `src/runtime/investigation/investigation.rs`, `src/runtime/investigation/graph.rs`, `src/runtime/orchestration/engine.rs`.

## Mutation Problems

Inspect the full path: `resolve()` → tool `run()` → `PendingAction` payload → `execute_approved()` → `handle_approve()`. Path rejection lives in `resolver.rs`; proposal validation lives in the tool; approval success or failure branching lives in `engine.rs`. For shell commands, verify `is_permitted_shell_command()` returns true for the command in `prompt_analysis.rs`. Code: `src/runtime/project/resolver.rs`, `src/tools/edit_file.rs`, `src/tools/write_file.rs`, `src/tools/shell.rs`, `src/runtime/orchestration/engine.rs`.

## Session and Restore Issues

Session data lives at `<thunk-data-dir>/data/sessions.db`. Schema is v3. `ActiveSession::open_or_restore()` loads the most recent session matching the current `project_root`. Restored anchor state (`last_read_file`, `last_search_query`, `last_search_scope`) comes from the `sessions` table. Code: `src/app/session.rs`, `src/storage/session/store.rs`, `src/storage/session/schema.rs`.

## Useful Test Entry Points

- Retrieval and scope: `src/runtime/tests/investigation.rs`, `src/runtime/tests/path_scope.rs`, `src/runtime/tests/investigation_modes.rs`, `src/runtime/tests/investigation_inline.rs`
- Search guardrails: `src/runtime/tests/search_guardrails.rs`, `src/runtime/tests/search_budget.rs`
- Read bounds: `src/runtime/tests/read_bounds.rs`
- Tool surfaces: `src/runtime/tests/tool_surface.rs`
- Approval: `src/runtime/tests/approval.rs`
- Answer finalization and protocol failures: `src/runtime/tests/finalization.rs`, `src/runtime/tests/tool_round.rs`
- Git tool isolation: `src/runtime/tests/git_acquisition.rs`
- Project snapshot: `src/runtime/tests/project_snapshot.rs`
- Integration: `src/runtime/tests/integration_misc.rs`, `src/runtime/tests/external_repo_fixtures.rs`
