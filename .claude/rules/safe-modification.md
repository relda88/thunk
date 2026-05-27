# Safe Modification Checklists

## Adding a New Tool

1. Add a variant to `ToolInput` in `src/tools/types.rs` and a matching `ToolOutput` variant.
2. Add a variant to `ResolvedToolInput` in `src/runtime/project/resolved_input.rs`.
3. Add a resolution arm in `resolve()` in `src/runtime/project/resolver.rs`.
4. Implement the `Tool` trait in `src/tools/<name>.rs`.
   - Read-only tools: `ExecutionKind::Immediate`, implement only `run()`.
   - Mutating tools: `ExecutionKind::RequiresApproval`, implement both `run()` (returns `Approval`) and `execute_approved()`.
5. Register the tool:
   - Root-independent tools: add to `default_registry()` in `src/tools/mod.rs`.
   - Root-dependent tools: add to `ToolRegistry::with_project_root()` in `src/tools/registry.rs`.
6. Add a `SurfaceTool` variant (read-only tools only) in `src/runtime/investigation/tool_surface.rs`.
   - Add it to the appropriate `*_TOOLS` constant.
   - Add an arm in `SurfaceTool::from_input()` and `SurfaceTool::name()`.
   - Mutation tools (`RequiresApproval`) must return `None` from `from_input()` and must appear in `mutation_tool_names()` for `MutationEnabled` only.
7. Add parse support in `src/runtime/protocol/tool_codec/tool_parser.rs`.
8. Add render support in `src/runtime/protocol/tool_codec/tool_renderer.rs`.
9. Add the tool call syntax to `format_instructions()` in `tool_renderer.rs`.
   - The `debug_assert!` in `build_system_prompt()` will catch missing entries at test time.
10. If the tool requires `&mut` state not available in `ToolRegistry::dispatch()` (e.g., `LspManager`), add an intercept in `run_tool_round()` in `src/runtime/orchestration/tool_round.rs` before the `registry.dispatch()` call.
11. Add a unit test in the new tool file and an integration test in `src/runtime/tests/`.

## Changing Retrieval Behavior

1. Identify which of the three layers needs to change:
   - **Candidate classification**: `InvestigationState::record_search_results()` in `src/runtime/investigation/investigation.rs`.
   - **Read acceptance**: `InvestigationState::record_read_result()` in the same file.
   - **Answer admission**: the answer-guard branches in `run_turns_with_initial_reads()` in `src/runtime/orchestration/engine.rs`.
2. If adding a new gate in `record_read_result()`, follow the `_correction_issued` bool pattern — fire each correction exactly once per turn.
3. If changing `evidence_ready()`, ensure it remains the single source of truth for evidence state; search text alone must never satisfy it.
4. If adding a new `InvestigationMode`, add it to `detect_investigation_mode()` in priority order, add a case in `best_candidate_for_mode()`, and add a corresponding gate in `record_read_result()`.
5. Update both candidate classification and answer admission together. Changing only one layer creates false terminals or false admissions.
6. New `InvestigationState` fields must reset in `new()` (the large initializer).
7. Add an integration test in `src/runtime/tests/` that would have caught the regression.

## Changing Mutation Behavior

1. Do not make a mutating tool `Immediate`. Mutations are designed around `PendingAction` + `execute_approved()`.
2. Keep `spec().execution_kind` aligned with the actual `ToolRunResult` returned by `run()`. The `debug_assert!` in `tool_round.rs` checks this at test time.
3. Approval-time revalidation is required in `execute_approved()`:
   - `EditFileTool`: recheck that the search text still exists in the current file contents.
   - `WriteFileTool`: recheck path validity and parent existence.
4. After a successful mutation, `handle_approve()` must commit the tool result, invalidate the project snapshot cache, and end with `mutation_complete_final_answer()`. Do not re-enter the backend.
5. After a rejected mutation, `handle_reject()` must inject a tool error and end with `rejection_final_answer()`. Do not re-enter the backend.
6. If a new tool can affect project structure, add snapshot cache invalidation in the approval success branch of `engine.rs`.
7. Shell commands are gated by `is_permitted_shell_command()` — only `cargo` is permitted. Do not weaken this allowlist without updating the invariant documentation.
