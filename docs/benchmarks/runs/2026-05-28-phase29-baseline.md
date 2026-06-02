# Benchmark Run — 2026-05-28 — Phase 29 Baseline
Date: 2026-05-28
Version: 0.14.53
Backend: openai
Model: gpt-4o-mini
Machine: MacBook Air M2, 8GB RAM

---

## Context

Full regression suite run at the close of Phase 29. Phase 29 delivered multi-file investigation via InvestigationGraph with petgraph (29.1), dynamic useful_candidate_reads_target (29.2), persistent LspManager session infrastructure (29.3), lsp_definition tool wiring (29.4), runtime-seeded LSP definition dispatch (29.5), declaration-site coordinate selection (29.6), integration test suite (29.7), hover context enrichment (29.8), post-edit diagnostics injection (29.9), /lsp status slash command (29.10), LSP warning resolution (29.11), and absolute path fix (29.12). This is the first full suite run since Phase 28 (Windows/ollama). All 25 tests run with gpt-4o-mini via OpenAI on Mac. LSP tests (17–22) run against the thunk codebase with lsp.enabled = true and rust-analyzer installed. Sandbox tests (1–16, 20–21, 23–25) run against the sandbox Python project.

---

## Key Behaviors Being Measured

- Investigation correctness: DefinitionLookup, UsageLookup, InitializationLookup, CallSiteLookup, General modes
- LSP definition seeding: runtime-seeded lsp_definition on DefinitionLookup turns (29.5)
- LSP declaration-site coordinate selection (29.6)
- Hover context enrichment after successful lsp_definition (29.8)
- Post-edit diagnostics injection on .rs files (29.9)
- /lsp status slash command: pre-session and post-session states (29.10)
- Absolute path rendering: lsp_definition and hover must show project-relative paths (29.12)
- File path scope fallback: resolve_scope() falls back to parent directory for file paths (29.5 fix)
- Candidate read limit behavior on broad queries
- Direct read detection for filenames
- Mutation approval flow with diff rendering
- Anchor follow-up reads
- Git read-only surface enforcement
- /git branch, /ls slash commands (28.2, 28.4)
- Session restore across restart

---

## Results

| Version | Date | Backend | Scenario | Prompt / action | Expected behavior | Observed behavior | Tool rounds | Answer mode | Pass | Notes | Source |
|---------|------|---------|----------|-----------------|-------------------|-------------------|-------------|-------------|------|-------|--------|
| 0.14.53 | 2026-05-28 | openai | Initialization lookup | Find where logging is initialized in sandbox/ | Identify correct init file | Searched, read z_init_target.py and logging_init.py (2 useful reads), hit candidate_read_limit_exhausted before synthesis. Terminal InsufficientEvidence. | 4 | RuntimeTerminal (InsufficientEvidence) | FAIL | Regression from Phase 28 PASS. Dynamic read target (29.2) not raising limit for InitializationLookup with 3 initialization candidates. useful_candidate_reads_target stayed at 2. Needs investigation as 29.14. | Test 1 |
| 0.14.53 | 2026-05-28 | openai | Definition lookup | Find where TaskStatus is defined in sandbox/ | Locate enum definition | LSP seeded on Python file, rust-analyzer returned empty. Model entered recovery loop re-reading enums.py 6 times. Tool limit reached. | 17 | ToolLimitReached | FAIL | New regression from Phase 29 LSP seeding. .rs extension check not preventing seeding on Python files. Model confused by LSP empty result on Python. Needs fix as 29.14. | Test 2 |
| 0.14.53 | 2026-05-28 | openai | Usage lookup (multi) | Find where TaskStatus is used in sandbox/ | Identify multiple usage sites | Read commands.py and task.py (2 useful reads), hit candidate_read_limit_exhausted. Terminal InsufficientEvidence. | 4 | RuntimeTerminal (InsufficientEvidence) | FAIL | Regression from Phase 28 PARTIAL. Dynamic read target not raising for broad UsageLookup. useful_candidate_reads_target stayed at 2 despite 6 candidates. | Test 3 |
| 0.14.53 | 2026-05-28 | openai | Call-site lookup | Find where load_config is called in sandbox/ | Identify call site in main.py | Correctly searched, read main.py, accurate answer identifying build_services and config_path argument. | 2 | ToolAssisted | PASS | Clean call-site lookup. CallSiteLookup mode confirmed working. | Test 4 |
| 0.14.53 | 2026-05-28 | openai | Call-site lookup | Find where init_logging is called in sandbox/ | Identify call site in main.py | Correctly searched, read main.py, accurate answer. | 2 | ToolAssisted | PASS | Clean call-site lookup. Consistent with Test 4. | Test 5 |
| 0.14.53 | 2026-05-28 | openai | Usage lookup (global) | Find where TaskRepository is used in sandbox/ | List usage locations | Read test_repository.py and main.py (2 useful reads), hit candidate_read_limit_exhausted. Terminal InsufficientEvidence. | 4 | RuntimeTerminal (InsufficientEvidence) | FAIL | Regression from Phase 28 PARTIAL. Same dynamic read target issue as Tests 1 and 3. 5 candidates, target stayed at 2. | Test 6 |
| 0.14.53 | 2026-05-28 | openai | General search | Find where completed tasks are filtered in sandbox/ | Identify filtering logic | Correctly searched, read task_service.py, accurate detailed answer covering completed_tasks, list_tasks, and _filter_by_status. | 2 | ToolAssisted | PASS | Clean general search. Strong synthesis. | Test 7 |
| 0.14.53 | 2026-05-28 | openai | File understanding | Find what task_service.py does in sandbox/ | Direct read of task_service.py, no search | Direct read triggered via filename detection. Accurate summary of all TaskService methods. | 1 | ToolAssisted | PASS | Direct read working. Answer not hidden behind Ctrl+O. | Test 8 |
| 0.14.53 | 2026-05-28 | openai | Direct read | Read sandbox/main.py | Return file contents, Ctrl+O to expand | Direct read triggered, file content behind Ctrl+O hint as designed. Zero model involvement. | 1 | ToolAssisted | PASS | Direct read working correctly. | Test 9 |
| 0.14.53 | 2026-05-28 | openai | Mutation (create) | Create sandbox/baseline_test.txt | Approval flow, file created | Correct approval flow, file created. cargo test proposed after write, rejected intentionally. | 1 | ToolAssisted | PASS | Mutation create flow working. | Test 10 |
| 0.14.53 | 2026-05-28 | openai | Mutation (edit) | Edit sandbox/baseline_test.txt change hello world to hello thunk | Approval flow, file edited with diff | Runtime seeded edit_file directly. Diff rendered correctly (- hello world / + hello thunk). Edit approved. cargo test proposed, rejected. | 1 | ToolAssisted | PASS | Simple edit seeding working. Diff rendering correct. | Test 11 |
| 0.14.53 | 2026-05-28 | openai | Anchor follow-up | Read sandbox/config.py → Read that again → Open that again | Re-read from anchor | First read showed Ctrl+O hint. Both follow-up reads resolved from anchor correctly. | 1/1/1 | ToolAssisted | PASS | Anchor resolution working correctly all three times. | Test 12 |
| 0.14.53 | 2026-05-28 | openai | Git read-only | git status → git diff → git | git tools fire, bare git answered directly | git_status and git_diff both fired correct tools. Bare "git" answered directly from context. No shell attempt. | 1/1/0 | ToolAssisted/ToolAssisted/Direct | PASS | Git read-only surface working. Bare git handled gracefully. | Test 13 |
| 0.14.53 | 2026-05-28 | openai | Definition + explain | Find where JsonFileStore is defined in sandbox/ and what it does | Locate and describe class | LSP seeded on Python file, returned empty (expected — Python not supported). Fell through to read_file. Read file_store.py, accurate description of read_records and write_records. | 3 | ToolAssisted | PASS | LSP graceful fallback to read_file working correctly for Python files. Accurate answer. | Test 14 |
| 0.14.53 | 2026-05-28 | openai | Usage lookup | Find where ArgumentParser is used in sandbox/ | Identify usage location | Read parser.py, non-candidate read of models/enums.py rejected correctly, correction fired, answer synthesized from parser.py. Accurate. | 3 | ToolAssisted | PASS | Non-candidate read rejection working. Answer correct. | Test 15 |
| 0.14.53 | 2026-05-28 | openai | File path scope fallback (29.5) | Find where TaskStatus is defined in sandbox/models/enums.py | Search scoped to parent dir, accurate answer | resolve_scope() fell back to parent directory sandbox/models/. Search fired (5 matches), LSP seeded on Python file (empty result expected), read enums.py, accurate answer. | 3 | ToolAssisted | PASS | 29.5 file-path scope fix confirmed working. Python LSP graceful fallback working. | Test 16 |
| 0.14.53 | 2026-05-28 | openai | LSP Rust definition (29.5–29.8) | Where is InvestigationGraph defined? (thunk codebase) | lsp_definition_seeded trace fires, correct line returned, hover injected, relative path, accurate answer | lsp_definition_seeded at line=21 col=19. lsp_definition returned src/runtime/investigation/graph.rs line 21. lsp_hover_injected fired. read_file followed. Accurate answer. All paths relative. | 3 | ToolAssisted | PASS | Full Phase 29 LSP stack working: seeding (29.5), declaration coords (29.6), hover enrichment (29.8), relative paths (29.12). rust-analyzer warm: 4.2s response. | Test 17 |
| 0.14.53 | 2026-05-28 | openai | /lsp status pre-session (29.10) | /lsp status (fresh session) | "no active session" + probe report | "LSP enabled — no active session (not yet started or crashed)" with probe report showing both rust-analyzer binaries ready. | 0 | N/A | PASS | 29.10 health reporting correct. Pre-session state accurate. rust-analyzer 1.92.0 detected. | Test 18 |
| 0.14.53 | 2026-05-28 | openai | /lsp status post-session (29.10) | /lsp status (after Test 17) | "session alive" + probe report | "LSP running — rust-analyzer active, session alive" with probe report. Session persisted correctly across turns. | 0 | N/A | PASS | 29.10 session state accurate. Persistent session infrastructure (29.3) confirmed working. | Test 19 |
| 0.14.53 | 2026-05-28 | openai | Compound definition + usage | Find where TaskRepository is defined and where it is used in sandbox/ | Read definition and usage files, accurate compound answer | Read test_repository.py and main.py (2 useful reads), hit candidate_read_limit_exhausted. Terminal InsufficientEvidence. | 4 | RuntimeTerminal (InsufficientEvidence) | FAIL | Same dynamic read target regression as Tests 1, 3, 6. 5 candidates, target stayed at 2. Compound query needed 3+ reads. | Test 20 |
| 0.14.53 | 2026-05-28 | openai | File scope graceful fallback (29.5) | Find where JsonFileStore is defined in sandbox/main.py | Scope falls back to parent dir, finds definition in file_store.py | Scope injected as sandbox/main.py, fell back to sandbox/ parent. Search found 9 matches. LSP seeded on Python file (empty). Read file_store.py, accurate answer. | 3 | ToolAssisted | PASS | 29.5 file-path scope fix working. Symbol found in different file than scope. Accurate answer. | Test 21 |
| 0.14.53 | 2026-05-28 | openai | LSP Rust definition with hover (29.8) | Where is run_tool_round defined? (thunk codebase) | lsp_definition_seeded, correct definition returned, hover injected | Search found 17 matches. LSP not seeded — no declaration-site match found in search results (search truncated at 15, definition line not shown). Read tool_round.rs and investigation.rs, both rejected as non-definition-site. Terminal InsufficientEvidence. | 3 | RuntimeTerminal (InsufficientEvidence) | FAIL | LSP seeding not firing — declaration line not in truncated search results (15 shown of 17). Known limitation: seeding requires declaration match in shown results. | Test 22 |
| 0.14.53 | 2026-05-28 | openai | /git branch (28.2) | /git branch | Lists local branches | "git branch: dev" shown correctly as system message. | 0 | N/A | PASS | /git branch slash command working. | Test 23 |
| 0.14.53 | 2026-05-28 | openai | /ls command (28.4) | /ls src/runtime/ | Lists directory contents | Listed 6 dirs and 6 files in src/runtime/ correctly. | 0 | N/A | PASS | /ls slash command working. | Test 24 |
| 0.14.53 | 2026-05-28 | openai | Post-edit diagnostics (29.9) | Edit sandbox/main.py adding a comment line, approve | Edit approved, no LSP diagnostics on Python file | Edit seeded directly. Diff rendered correctly. Edit approved. No lsp_diagnostics block injected (Python file — correct). cargo test proposed, rejected. | 1 | ToolAssisted | PASS | 29.9 diagnostics correctly skips non-.rs files. Mutation flow working. | Test 25 |

---

## Summary

| Result | Count |
|--------|-------|
| PASS | 18 |
| PARTIAL | 0 |
| FAIL | 7 |
| **Total** | **25** |

---

## Known Issues

- **Tests 1, 3, 6, 20 — dynamic read target not raising (29.2 regression):** `useful_candidate_reads_target` stays at 2 despite multiple candidates existing for InitializationLookup and broad UsageLookup turns. Phase 27 and 28 baselines had these as PASS or PARTIAL — these are now FAIL. Root cause: `compute_read_target()` signals not triggering target raise. Needs investigation as Phase 29.14.
- **Test 2 — LSP seeding on Python files causes model loop:** `lsp_definition_seeded` fires on Python `.py` files despite rust-analyzer not supporting them. LSP returns empty, but the model enters a recovery loop re-reading the same file repeatedly instead of falling through cleanly. The `.rs` extension guard in the seeding block is not preventing dispatch for Python files. Needs fix as Phase 29.14.
- **Test 22 — LSP seeding not firing when declaration line truncated:** `search_code` shows 15 of 17 matches. The declaration line for `run_tool_round` is not in the shown results, so `is_declaration_line()` finds no match and seeding falls back to the first match (a call site), which returns no definition. Known limitation of search truncation at 15 results. Lower priority — not a regression.
- **LSP cold start latency:** First rust-analyzer query per session takes 20–25 seconds while the server indexes the project. Subsequent queries in the same session respond in 3–5 seconds. Expected behavior — documented for user awareness.
- **Python LSP not supported:** All LSP calls on `.py` files return empty (expected — rust-analyzer handles Rust only). Graceful fallback to `read_file` works correctly in Tests 14, 16, 21. The seeding guard needs to check file extension before dispatching (Test 2 regression).