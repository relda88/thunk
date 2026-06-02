# Benchmark Run — 2026-05-29 — Phase 30 Baseline

Date: 2026-05-29
Version: 0.15.55
Backend: openai
Model: gpt-4o-mini
Machine: MacBook Air M2, 8GB RAM

---

## Context

Phase 30 close benchmark. Validates symbol index integration, on-demand
build trigger, investigation graph pre-seeding, and all pre-existing
behaviors from Phase 29. First query in each session triggers index build
(index: empty → building → N symbols indexed). Index hit/miss logged via
tracing. 25 tests covering investigation modes, mutations, anchors, git
commands, slash commands, and LSP status.

---

## Key Behaviors Being Measured

- InitializationLookup: runtime reads init site after recovery dispatch
- DefinitionLookup (small codebase): rg finds definition in shown matches
- DefinitionLookup (large codebase): index hit promotes candidate, LSP confirms line
- UsageLookup: reads 2 usage candidates + definition bypass
- CallSiteLookup: finds call site directly from search results
- General/direct read: reads file without search
- Mutation pipeline: write_file, edit_file, approval gate
- Anchor resolution: "read that again", "open that again"
- Git commands: /git status, /git diff, /git branch
- Slash commands: /ls, /lsp status
- Index build trigger: fires after first search_code in session
- Index miss: falls through silently to rg + LSP path

---

## Results

| Version | Date | Backend | Scenario | Prompt / action | Expected behavior | Observed behavior | Tool rounds | Answer mode | Pass | Notes |
|---------|------|---------|----------|-----------------|-------------------|-------------------|-------------|-------------|------|-------|
| 0.15.55 | 2026-05-29 | openai | InitializationLookup, scoped, truncated | Find where logging is initialized in sandbox/ | Reads init site, correct answer | Read z_init_target.py then logging_init.py via recovery, correct answer | 3 | ToolAssisted | PASS | useful_target=2, recovery dispatched next unread candidate |
| 0.15.55 | 2026-05-29 | openai | DefinitionLookup, scoped, truncated, index miss | Find where TaskStatus is defined in sandbox/ | Reads enums.py, correct answer | index_miss, read enums.py directly, correct answer | 2 | ToolAssisted | PASS | index miss falls through to rg correctly |
| 0.15.55 | 2026-05-29 | openai | UsageLookup, scoped, truncated | Find where TaskStatus is used in sandbox/ | Reads 2 usage candidates + definition bypass | Read commands.py, task.py, enums.py (bypass), correct answer | 4 | ToolAssisted | PASS | definition_site_dispatch_bypass fired |
| 0.15.55 | 2026-05-29 | openai | CallSiteLookup, scoped, no truncation | Find where load_config is called in sandbox/ | Reads call site file, correct answer | Read main.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | CallSiteLookup, scoped, no truncation | Find where init_logging is called in sandbox/ | Reads call site file, correct answer | Read main.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | UsageLookup, scoped, no truncation | Find where TaskRepository is used in sandbox/ | Reads 2 usage candidates + definition bypass | Read test_repository.py, main.py, repository.py (bypass), correct answer | 4 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | General, scoped, semantic query | Find where completed tasks are filtered in sandbox/ | Reads relevant file, correct answer | Read task_service.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | General, direct file query | Find what task_service.py does in sandbox/ | Reads file, describes it | Read task_service.py, correct description | 1 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | General, direct read | Read sandbox/main.py | Reads file, no search | Read main.py directly, no search | 1 | ToolAssisted | PASS | reason=direct_read |
| 0.15.55 | 2026-05-29 | openai | Mutation, write + approve | Create sandbox/baseline_test.txt | Creates file, awaits approval | write_file dispatched, approval required, created on approve | 1 | ToolAssisted | PASS | cargo test rejected as expected |
| 0.15.55 | 2026-05-29 | openai | Mutation, edit + approve | Edit sandbox/baseline_test.txt change hello world to hello thunk | Edits file, awaits approval | edit_file dispatched, diff shown, replaced on approve | 1 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | Anchor resolution, multi-turn | Read sandbox/config.py → Read that again → Open that again | Re-reads same file on anchor match | anchor_resolved correctly on both follow-ups | 1 each | ToolAssisted | PASS | anchor_prompt_matched kind=last_read_file both turns |
| 0.15.55 | 2026-05-29 | openai | Git commands, multi-turn | git status → git diff → git (ambiguous) | Status and diff succeed, ambiguous handled gracefully | git_status clean, git_diff empty, ambiguous answered directly | 1 each | ToolAssisted / Direct | PASS | |
| 0.15.55 | 2026-05-29 | openai | DefinitionLookup, scoped, no truncation, index miss | Find where JsonFileStore is defined in sandbox/ and what it does | Reads definition file, correct answer | index_miss, read file_store.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | UsageLookup, scoped, low match count | Find where ArgumentParser is used in sandbox/ | Reads usage file, correct answer | Read parser.py, non-candidate read rejected correctly, correct answer | 3 | ToolAssisted | PASS | non_candidate_read_rejected fired, recovery corrected |
| 0.15.55 | 2026-05-29 | openai | DefinitionLookup, file-scoped | Find where TaskStatus is defined in sandbox/models/enums.py | Reads scoped file, correct answer | index_miss, read enums.py, correct answer | 2 | ToolAssisted | PASS | scope injected as file path |
| 0.15.55 | 2026-05-29 | openai | DefinitionLookup, no scope, index hit via LSP | Where is InvestigationGraph defined? | Reads graph.rs, correct answer | index_miss, LSP seeded graph.rs line 21, read accepted, correct answer | 3 | ToolAssisted | PASS | LSP path used; index miss fell through correctly |
| 0.15.55 | 2026-05-29 | openai | LSP status, fresh session | /lsp status (fresh session) | Shows LSP state + probe report | LSP enabled, no active session, probe report shown | — | SystemMessage | PASS | |
| 0.15.55 | 2026-05-29 | openai | LSP status, after query | /lsp status (after Test 17) | Shows LSP running | LSP running, rust-analyzer active, session alive | — | SystemMessage | PASS | |
| 0.15.55 | 2026-05-29 | openai | UsageLookup + DefinitionLookup, combined | Find where TaskRepository is defined and where it is used in sandbox/ | Reads usage candidates + definition, correct answer | Read test_repository.py, main.py, repository.py (bypass), correct answer | 4 | ToolAssisted | PASS | |
| 0.15.55 | 2026-05-29 | openai | DefinitionLookup, file-scoped, index miss | Find where JsonFileStore is defined in sandbox/main.py | Reads definition file ignoring wrong scope, correct answer | index_miss, read file_store.py, correct answer | 2 | ToolAssisted | PASS | scope was main.py but definition found in file_store.py |
| 0.15.55 | 2026-05-29 | openai | DefinitionLookup, no scope, truncated, index hit second query | Where is run_tool_round defined? | Index hit on second query answers correctly | Q1: index_miss → InsufficientEvidence. Q2: index_hit → LSP line 188 → correct answer | 3 (Q2) | RuntimeTerminal (Q1) / ToolAssisted (Q2) | PARTIAL | First query triggers index build. Second query in same session gets index hit. Known limitation: index not built before first query in session. |
| 0.15.55 | 2026-05-29 | openai | Slash command, git branch | /git branch | Shows current branch | dev | — | SystemMessage | PASS | |
| 0.15.55 | 2026-05-29 | openai | Slash command, list dir | /ls src/runtime/ | Lists directory contents | 7 dirs, 6 files shown correctly | — | SystemMessage | PASS | |
| 0.15.55 | 2026-05-29 | openai | Mutation, edit with read + approve | Edit sandbox/main.py adding a comment line, approve the edit | Reads file, edits, awaits approval, applies on approve | list_dir → read_file → edit_file approved, comment added | 3 | ToolAssisted | PASS | |

---

## Summary

| Result | Count |
|--------|-------|
| PASS    | 24 |
| PARTIAL | 1 |
| FAIL    | 0 |
| **Total** | **25** |

---

## Known Issues

- **Test 22 (run_tool_round, first query)**: DefinitionLookup on a heavily-referenced Rust function fails on the first query in a session because the index hasn't been built yet. The on-demand build triggers after the first search_code turn, so the second query in the same session gets an index hit and answers correctly. Root fix would be eager index build at session start, or persisting the index across sessions so it's available immediately. Tracked for Phase 31+ consideration.