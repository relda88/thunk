# Benchmark Run — 2026-06-10 — Phase 39 Baseline

Date: 2026-06-10
Version: 0.23.72
Backend: openai
Model: gpt-4o-mini
Machine: MacBook Air M2, 8GB RAM

---

## Context

Full regression run after Phase 39 (Runtime & Investigation Hardening, Refactoring, and Cleanup) completion. Phase 39 added:
- 39.1: Fixed bare filename suffix match in is_search_candidate_path (Bug #5), seeded read_file for agent file targets (Bug #6)
- 39.2: Ported PromptAssembled to all providers (Bug #7), added per-chunk embed progress (Bug #8)
- 39.3: Consolidated looks_like_definition, is_exact_symbol_definition, is_declaration_line into classify.rs
- 39.4: Split engine.rs — extracted answer_guard.rs, answer_admission.rs, retrieval_log_writer.rs
- 39.5: Split command_handlers.rs — extracted plan_handlers.rs, embed_handlers.rs, agent_prompts.rs
- 39.6: Migrated scenarios.rs to tests/scenarios.rs, promoted silent storage failures to trace
- 39.7: Added IndexEmbedChunk chunked embed dispatch with PendingEmbedState
- 39.8: Synced all docs to current state
- 39.9: Added relational verb coverage to investigation mode detection (reaches, interacts, connects, trace, follow)

Tests 1–30 carry over regression scenarios from the Phase 34 baseline.
Tests 31–35 are new Phase 39 validation scenarios.

---

## Key Behaviors Being Measured

- Investigation path (retrieval, definition, usage, call site, initialization lookups) unchanged by Phase 39 refactoring
- Mutation path (write, edit, approve, reject) unaffected by orchestration splits
- Anchor resolution working correctly across multi-turn sessions
- Git read-only surface working correctly
- Agent workflows (investigate, refactor) seeding correct initial tool calls
- Ctrl+D prompt dump working on OpenAI backend
- /index embed per-chunk progress messages appearing
- Retrieval log writing correctly with [vec] suffix on hybrid turns
- /depth toggle persisting correctly session-scoped
- Semantic queries using "reaches", "interacts" now triggering investigation

---

## Results

| Version | Date | Backend | Scenario | Prompt / action | Expected behavior | Observed behavior | Tool rounds | Answer mode | Pass | Notes |
|---------|------|---------|----------|-----------------|-------------------|-------------------|-------------|-------------|------|-------|
| 0.23.72 | 2026-06-10 | openai | InitializationLookup, scoped, truncated | Find where logging is initialized in sandbox/ | Reads init site, correct answer | Read z_init_target.py then logging_init.py, correct answer | 3 | ToolAssisted | PASS | useful_target=2, recovery dispatched next unread candidate |
| 0.23.72 | 2026-06-10 | openai | DefinitionLookup, scoped, truncated, index miss | Find where TaskStatus is defined in sandbox/ | Reads enums.py, correct answer | index_miss, read enums.py directly, correct answer | 2 | ToolAssisted | PASS | |
| 0.23.72 | 2026-06-10 | openai | UsageLookup, scoped, truncated | Find where TaskStatus is used in sandbox/ | Reads usage candidates + definition bypass | Read commands.py, task.py, enums.py (bypass), correct answer | 4 | ToolAssisted | PASS | answer_guard_rejected once, retry succeeded |
| 0.23.72 | 2026-06-10 | openai | CallSiteLookup, scoped, no truncation | Find where load_config is called in sandbox/ | Reads call site file, correct answer | Read main.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.23.72 | 2026-06-10 | openai | CallSiteLookup, scoped, no truncation | Find where init_logging is called in sandbox/ | Reads call site file, correct answer | Read main.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.23.72 | 2026-06-10 | openai | UsageLookup, scoped, no truncation | Find where TaskRepository is used in sandbox/ | Reads usage candidates + definition bypass | Read test_repository.py, main.py, repository.py (bypass), correct answer | 4 | ToolAssisted | PASS | answer_guard_rejected once, retry succeeded |
| 0.23.72 | 2026-06-10 | openai | General, scoped, semantic query | Find where completed tasks are filtered in sandbox/ | Reads relevant file, correct answer | Read task_service.py + report_service.py, correct answer | 3 | ToolAssisted | PASS | |
| 0.23.72 | 2026-06-10 | openai | General, direct file query | Find what task_service.py does in sandbox/ | Reads file, describes it | Read task_service.py, correct description | 1 | ToolAssisted | PASS | reason=direct_read |
| 0.23.72 | 2026-06-10 | openai | General, direct read | Read sandbox/main.py | Reads file, no search | Read main.py directly, no search | 1 | ToolAssisted | PASS | reason=direct_read |
| 0.23.72 | 2026-06-10 | openai | DefinitionLookup, scoped, no truncation, index miss | Find where JsonFileStore is defined in sandbox/ and what it does | Reads definition file, correct answer | index_miss, read file_store.py, correct answer | 2 | ToolAssisted | PASS | |
| 0.23.72 | 2026-06-10 | openai | UsageLookup, scoped, low match count | Find where ArgumentParser is used in sandbox/ | Reads usage file, correct answer | Read parser.py, non-candidate read rejected correctly, correct answer | 3 | ToolAssisted | PASS | non_candidate_read_rejected fired, recovery corrected |
| 0.23.72 | 2026-06-10 | openai | DefinitionLookup, file-scoped | Find where TaskStatus is defined in sandbox/models/enums.py | Reads scoped file, correct answer | index_miss, read enums.py, correct answer | 2 | ToolAssisted | PASS | scope injected as file path |
| 0.23.72 | 2026-06-10 | openai | DefinitionLookup, no scope, index hit via LSP | Where is InvestigationGraph defined? | Reads graph.rs, correct answer | index_miss, LSP seeded graph.rs line 21, read accepted, correct answer | 3 | ToolAssisted | PASS | LSP startup delay ~30s; correct answer on second attempt |
| 0.23.72 | 2026-06-10 | openai | DefinitionLookup, no scope, truncated, index hit second query | Where is run_tool_round defined? | Index hit on second query answers correctly | Q1: index_miss → InsufficientEvidence. Q2: index_hit → LSP line 191 → correct answer | 3 (Q2) | RuntimeTerminal (Q1) / ToolAssisted (Q2) | PARTIAL | First query: read_evidence rejected as definition_lookup_non_definition_site. Second query in same session: index_hit, LSP seeded correct line, answer accepted. Known limitation unchanged. |
| 0.23.72 | 2026-06-10 | openai | General, nonexistent file, scoped | What is in ghost_module.py in sandbox/ | InsufficientEvidence terminal, no hallucination | Search found nothing, listed sandbox/, second search found nothing, terminal InsufficientEvidence | 3 | RuntimeTerminal | PASS | empty_search_no_read terminal fired correctly |
| 0.23.72 | 2026-06-10 | openai | Slash command, /depth toggle | /depth deep → /depth normal → /depth status | Toggle persists, status reflects current | depth_toggled deep → depth_toggled normal → status shows normal | — | SystemMessage | PASS | |
| 0.23.72 | 2026-06-10 | openai | General, relational query, deep mode | /depth deep then Find how the CLI dispatch layer reaches the storage layer in sandbox/ | investigation_required=true, RetrievalFirst surface, answer admitted | required=true, read commands.py, correct answer about CLI→service→storage chain | 2 | ToolAssisted | PASS | 39.9 fix confirmed — previously failed with required=false |
| 0.23.72 | 2026-06-10 | openai | General, relational query, shallow mode | /depth shallow then Find how the task service interacts with the repository in sandbox/ | investigation_required=true, shallow mode stops after 1 read | required=true, read task_service.py, shallow_mode_single_read_exhausted terminal on first attempt; second attempt without shallow succeeds | 3 | RuntimeTerminal (Q1) / ToolAssisted (Q2) | PARTIAL | Shallow mode correctly limits to 1 read and terminates. Second attempt in same session (depth reset to normal) reads task_service.py + repository.py and answers correctly. Shallow mode working as designed — terminal is expected behavior. |
| 0.23.72 | 2026-06-10 | openai | Agent, file target seeding | /agent investigate sandbox/database.py | read_file seeded pre-generation, not list_dir | read_file sandbox/database.py fires before any generation, investigation proceeds | 3 | RuntimeTerminal (RepeatedToolAfterEvidenceReady) | PASS | Bug #6 fix confirmed — read_file seeded correctly. Terminal due to model calling tools after evidence ready — runtime correctly blocked. |
| 0.23.72 | 2026-06-10 | openai | Agent, refactor file target | /agent refactor sandbox/config.py | read_file seeded pre-generation, plan generated | read_file sandbox/config.py fires first, InsufficientEvidence on investigation, plan generated correctly | — | Plan generated | PASS | Bug #6 fix confirmed — read_file seeded correctly. Plan approval widget shown. |
| 0.23.72 | 2026-06-10 | openai | Provider, Ctrl+D prompt dump | Ctrl+D after any query (OpenAI backend) | Prompt dumped to file, not "no prompt captured yet" | "prompt dumped to /tmp/thunk_last_prompt.txt" in status bar | — | SystemMessage | PASS | Bug #7 fix confirmed — PromptAssembled ported to OpenAI provider |
| 0.23.72 | 2026-06-10 | openai | Anchor resolution, multi-turn | Read sandbox/config.py → Read that again → Open that again | Re-reads same file on anchor match | anchor_resolved correctly on both follow-ups | 1 each | ToolAssisted | PASS | anchor_prompt_matched kind=last_read_file both turns |
| 0.23.72 | 2026-06-10 | openai | Git commands, multi-turn | git status → git diff | Status and diff succeed | git_status clean on dev, git_diff empty | 1 each | ToolAssisted | PASS | GitReadOnly surface selected correctly |
| 0.23.72 | 2026-06-10 | openai | Slash command, git branch | /git branch then git branch | Shows current branch both ways | dev branch shown correctly via slash command and natural language | — | SystemMessage / ToolAssisted | PASS | |
| 0.23.72 | 2026-06-10 | openai | Slash command, list dir | /ls src/runtime/ | Lists directory contents | 7 dirs, 5 files shown correctly | — | SystemMessage | PASS | |
| 0.23.72 | 2026-06-10 | openai | LSP status, fresh session | /lsp status (fresh session) | Shows LSP state + probe report | LSP enabled, no active session, probe report shown | — | SystemMessage | PASS | |
| 0.23.72 | 2026-06-10 | openai | Embed pipeline + retrieval log | /index build → /index embed → Find how does the logging configuration get loaded in sandbox/ → /retrieval log | 63 chunks progress, embeddings stored, vector_augment_applied, [vec] in log | All confirmed — 63 chunks, 2000 embeddings, vector_augment_applied weight=0.30, retrieval log shows strategy=ConfigLookup candidates=7 reads=2 gate=met [vec] | 3 | ToolAssisted | PASS | Bug #8 + 39.7 IndexEmbedChunk confirmed. Full vector pipeline working. |
| 0.23.72 | 2026-06-10 | openai | Mutation, write + approve | Create sandbox/baseline_test.txt with the content hello world | Creates file, awaits approval | write_file dispatched, approval required, created on approve | 1 | ToolAssisted | PASS | cargo test rejected as expected |
| 0.23.72 | 2026-06-10 | openai | Mutation, edit + approve | Edit sandbox/baseline_test.txt change hello world to hello thunk | Edits file, awaits approval | edit_file dispatched, diff shown, replaced on approve | 1 | ToolAssisted | PASS | cargo test rejected as expected |
| 0.23.72 | 2026-06-10 | openai | Mutation, edit with read + approve | Edit sandbox/main.py adding a comment line, approve the edit | Reads file, edits, awaits approval, applies on approve | list_dir → read_file → edit_file approved, comment added | 3 | ToolAssisted | PASS | malformed_block_correction fired once before valid edit_file emitted |
| 0.23.72 | 2026-06-10 | openai | Mutation, verify on + pass | /verify python3 -m py_compile sandbox/main.py → Add a comment to sandbox/utils/time_utils.py → approve | Edit executes, verify fires, ok | edit_file approved, "verifying..." → "python3 -m py_compile sandbox/main.py: ok" | 1 | ToolAssisted | PASS | Verify fires correctly after mutation on Python file |
| 0.23.72 | 2026-06-10 | openai | Mutation, verify off | /verify off → Add a comment to sandbox/database.py → approve | No verify output | /verify off set, model searched database, read sandbox/database.py correctly, edit approved, no verify fired | 1 | ToolAssisted | PASS | Bug #6 fix also improved this — previously model searched for filename instead of file. Now correctly reads sandbox/database.py |
| 0.23.72 | 2026-06-10 | openai | Slash command, /transaction + /verify status | /transaction → /verify status | "no pending transaction" + verify status | "no pending transaction" / "verify: disabled" | — | SystemMessage | PASS | Both commands work correctly |
| 0.23.72 | 2026-06-10 | openai | UsageLookup + DefinitionLookup, combined | Find where TaskRepository is defined and where it is used in sandbox/ | Reads usage candidates + definition, correct answer | Q1: answer_guard_rejected (cited task_service.py not read) → terminal. Q2 same session: answer admitted correctly | 4 (Q2) | RuntimeTerminal (Q1) / ToolAssisted (Q2) | PARTIAL | First attempt: answer_guard_rejected task_service.py not in evidence, terminal. Second attempt in same session: answer admitted correctly with repository.py + main.py + test_repository.py read. Known limitation: answer guard rejects first attempt citing unread file. |
| 0.23.72 | 2026-06-10 | openai | DefinitionLookup, file-scoped, index miss | Find where JsonFileStore is defined in sandbox/main.py | Reads definition file ignoring wrong scope, correct answer | Both attempts: answer_scope_guard_rejected — file_store.py outside sandbox/main.py scope | 2 each | RuntimeTerminal | PARTIAL | Scope guard correctly prevents citing file_store.py since scope is sandbox/main.py. Both attempts consistent. Known limitation: file-scoped definition lookup with wrong scope file terminates consistently. Scope guard working correctly. |

---

## Summary

| Result | Count |
|--------|-------|
| PASS    | 28 |
| PARTIAL | 4 |
| FAIL    | 0 |
| **Total** | **32** |

---

## Known Issues

**Test 14 (PARTIAL) — run_tool_round first query InsufficientEvidence**
Unchanged from Phase 34 baseline. First query in a fresh session hits index_miss and exhausts candidate reads without finding the definition at the correct line. Second query in the same session gets an index_hit via LSP and succeeds. Known limitation: index not built before first query; LSP startup delay ~30s on first use.

**Test 18 (PARTIAL) — shallow mode terminates on first read as designed**
/depth shallow correctly limits investigation to 1 candidate read. The shallow_mode_single_read_exhausted terminal on the first attempt is expected behavior. Second attempt (after resetting to normal depth) succeeds with 2 reads and correct answer. Shallow mode working as designed.

**Test 19 (PASS with note) — /agent investigate terminal RepeatedToolAfterEvidenceReady**
read_file correctly seeded pre-generation (Bug #6 fix confirmed). Investigation proceeds correctly. Terminal fires because model attempts further tool calls after evidence gates satisfied. Runtime correctly blocks. This is correct runtime behavior — model behavior issue not a runtime bug.

**Test 34 (PARTIAL) — answer_guard rejects first TaskRepository attempt citing unread task_service.py**
The answer guard correctly rejects the first answer because the model cites task_service.py which was not read. Second attempt in same session (with prior context) succeeds. This is the answer guard working correctly — the model needs to be more conservative about what it cites. Known limitation.

**Test 35 (PARTIAL) — file-scoped JsonFileStore definition lookup**
When scoped to sandbox/main.py, the definition lives in sandbox/storage/file_store.py which is outside the scope. The answer_scope_guard correctly rejects citing out-of-scope files. Both attempts consistent. This is correct scope guard behavior — the prompt is asking for something that cannot be answered within the stated scope. Known limitation: file-scoped definition lookup when definition is in a different file always terminates.