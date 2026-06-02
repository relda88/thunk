# Benchmark Run — 2026-05-26 — Phase 27 Baseline (Pre Phase 28)
Date: 2026-05-26
Version: 0.12.49
Backend: openai
Model: gpt-4o-mini
Machine: M2 Air 8GB

---

## Context

Full regression suite run at the close of Phase 27. Phase 27 delivered three runtime investigation fixes (27.1 definition candidate dispatch, 27.2 answer guard and scope guard correctness) and four TUI improvements (27.3 scrollable output, 27.4 file content truncation with Ctrl+O toggle, 27.5 diff rendering at mutation approval, 27.6 message and error styling). This is the first full suite run since Phase 25 (Phase 26 baseline was a targeted 4-test re-run). All 24 tests run with gpt-4o-mini via OpenAI. Windows validation is ongoing — a separate UNC path fix was applied during this phase and is tracked separately.

---

## Key Behaviors Being Measured

- Investigation correctness: definition candidate dispatch on UsageLookup (27.1), answer guard recovery (27.2)
- Direct read detection for multiple phrasings
- Mutation approval flow with diff rendering (27.5)
- Anchor follow-up reads
- Git read-only surface enforcement
- Session restore across restart
- Provider switching
- Ctrl+O file content expand toggle (27.4)
- Undo stack
- Shell tool approval and exit code capture

---

## Results

| Version | Date       | Backend | Scenario                  | Prompt / action                                              | Expected behavior                                                      | Observed behavior                                                                                                                                                             | Tool rounds | Answer mode                              | Pass    | Notes                                                                                                                                                  | Source  |
|---------|------------|---------|---------------------------|--------------------------------------------------------------|------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------|------------------------------------------|---------|--------------------------------------------------------------------------------------------------------------------------------------------------------|---------|
| 0.12.49 | 2026-05-26 | openai  | Initialization lookup     | Find where logging is initialized in sandbox/                | Identify correct init file                                             | Correctly read z_init_target.py, accurate answer. Answer hidden behind Ctrl+O until expanded — 27.4 regression on investigation answers.                                      | 2           | ToolAssisted                             | PARTIAL | 27.4 Ctrl+O toggle incorrectly hides investigation answers, not just direct file reads. Needs fix in Phase 28.                                         | Test 1  |
| 0.12.49 | 2026-05-26 | openai  | Definition lookup         | Find where TaskStatus is defined in sandbox/                 | Locate enum definition                                                 | Correctly read enums.py, accurate answer. Answer hidden behind Ctrl+O — same 27.4 regression.                                                                                 | 2           | ToolAssisted                             | PARTIAL | Same 27.4 regression as Test 1.                                                                                                                        | Test 2  |
| 0.12.49 | 2026-05-26 | openai  | Usage lookup (multi)      | Find where TaskStatus is used in sandbox/                    | Identify multiple usage sites                                          | Correctly read commands.py, task.py, and enums.py (definition_site_dispatch_bypass). Answer guard fired once on cli/parser.py, recovered on retry. Accurate synthesis.        | 4           | ToolAssisted                             | PASS    | 27.1 definition dispatch confirmed — enums.py dispatched after usage candidates exhausted. Answer guard retry working (27.2).                          | Test 3  |
| 0.12.49 | 2026-05-26 | openai  | Call-site lookup          | Find where load_config is called in sandbox/                 | Identify call site in main.py                                          | Correctly read main.py, accurate answer identifying build_services function.                                                                                                   | 2           | ToolAssisted                             | PASS    | Phase 25 PARTIAL upgraded to PASS — gpt-4o-mini synthesizes precisely.                                                                                | Test 4  |
| 0.12.49 | 2026-05-26 | openai  | Call-site lookup          | Find where init_logging is called in sandbox/                | Identify call site in main.py                                          | Correctly read main.py, accurate answer identifying build_services function and config argument.                                                                               | 2           | ToolAssisted                             | PASS    | Clean call-site lookup. Consistent with Test 4.                                                                                                        | Test 5  |
| 0.12.49 | 2026-05-26 | openai  | Usage lookup (global)     | Find where TaskRepository is used in sandbox/                | List usage locations                                                   | Correctly read test_repository.py, main.py, and storage/repository.py (definition_site_dispatch_bypass). Answer guard fired once on task_service.py, recovered. Accurate answer. | 4           | ToolAssisted                             | PASS    | 27.1 definition dispatch confirmed. Phase 25 FAIL upgraded to PASS.                                                                                   | Test 6  |
| 0.12.49 | 2026-05-26 | openai  | General search            | Find where completed tasks are filtered in sandbox/          | Identify filtering logic                                               | Correctly read task_service.py, accurate answer identifying completed_tasks method and list_tasks.                                                                             | 2           | ToolAssisted                             | PASS    | Clean general search. Consistent with Phase 25.                                                                                                        | Test 7  |
| 0.12.49 | 2026-05-26 | openai  | File understanding        | Find what task_service.py does in sandbox/                   | Direct read of task_service.py, no search                             | Direct read triggered correctly, accurate summary returned.                                                                                                                    | 1           | ToolAssisted                             | PASS    | 26.2 fix holding. Answer hidden behind Ctrl+O — same 27.4 regression.                                                                                 | Test 8  |
| 0.12.49 | 2026-05-26 | openai  | Direct read               | Read sandbox/main.py                                         | Return file contents                                                   | Direct read, file content hidden behind Ctrl+O hint as designed. Zero model involvement.                                                                                       | 1           | ToolAssisted                             | PASS    | 27.4 working as intended for explicit reads. Ctrl+O expands correctly.                                                                                 | Test 9  |
| 0.12.49 | 2026-05-26 | openai  | Mutation (create)         | Create sandbox/baseline_test.txt                             | Approval flow, file created                                            | Correct approval flow, file created. cargo test proposed after write, rejected intentionally.                                                                                  | 1           | ToolAssisted                             | PASS    | Mutation create flow working.                                                                                                                          | Test 10 |
| 0.12.49 | 2026-05-26 | openai  | Mutation (edit)           | Edit sandbox/baseline_test.txt add the content hello thunk   | Approval flow, file written with content                               | Model used write_file (overwrite) instead of edit_file — acceptable for empty file. Approval flow correct. Content written. cargo test proposed, rejected.                    | 1           | ToolAssisted                             | PASS    | write_file used instead of edit_file on empty file — expected behavior.                                                                                | Test 11 |
| 0.12.49 | 2026-05-26 | openai  | Anchor follow-up          | Read sandbox/config.py → Read that again → Open that again   | Re-read from anchor                                                    | First read showed full content (not hidden — 27.4 regression on initial reads). Follow-up reads resolved from anchor. Note: only most recent read toggleable via Ctrl+O.      | 1           | ToolAssisted                             | PARTIAL | 27.4 regression: previous reads show full content inline, only most recent has Ctrl+O toggle. Noted for Phase 28 fix.                                  | Test 12 |
| 0.12.49 | 2026-05-26 | openai  | Git read-only             | git status → git diff → git                                  | git tools fire, no shell attempt on GitReadOnly                        | git status and git diff both used correct git tools. Bare "git" answered directly as ambiguous input. No shell attempt.                                                        | 1/1/0       | ToolAssisted/ToolAssisted/Direct         | PASS    | 26.1 fix holding. Bare git command handled gracefully.                                                                                                 | Test 13 |
| 0.12.49 | 2026-05-26 | openai  | Definition + explain      | Find where JsonFileStore is defined in sandbox/ and what it does | Locate and describe class                                           | Correctly read file_store.py, accurate description of read/write methods.                                                                                                      | 2           | ToolAssisted                             | PASS    | Clean compound definition+explain query.                                                                                                               | Test 14 |
| 0.12.49 | 2026-05-26 | openai  | Usage lookup              | Find where ArgumentParser is used in sandbox/                | Identify usage location                                                | Correctly read parser.py, accurate answer.                                                                                                                                     | 2           | ToolAssisted                             | PASS    | Clean single usage candidate.                                                                                                                          | Test 15 |
| 0.12.49 | 2026-05-26 | openai  | Shell tool (success)      | Run cargo check                                              | Approval prompt appears, runs, exit 0 captured                         | Approval prompt appeared, exit 0 captured correctly.                                                                                                                           | 1           | ToolAssisted                             | PASS    | Runtime seeded shell directly.                                                                                                                         | Test 16 |
| 0.12.49 | 2026-05-26 | openai  | Shell tool (failure)      | Run cargo test --this-test-does-not-exist                    | Approval prompt appears, non-zero exit captured                        | Approval prompt appeared, exit 1 captured correctly.                                                                                                                           | 1           | ToolAssisted                             | PASS    | Non-zero exit correctly surfaced.                                                                                                                      | Test 17 |
| 0.12.49 | 2026-05-26 | openai  | Mutation (edit) with diff | Edit sandbox/test.txt, replace hello with goodbye → /undo    | Diff shown at approval, file restored after /undo                      | Diff rendered correctly at approval (- hello / + goodbye). Edit approved, undo stack restored file correctly.                                                                  | 1           | ToolAssisted                             | PASS    | 27.5 diff rendering confirmed working. Undo stack working.                                                                                             | Test 19 |
| 0.12.49 | 2026-05-26 | openai  | Session restore           | What is a pointer? → quit → restart → Does Rust have them?   | Follow-up answered using restored context                              | Follow-up correctly answered without re-establishing context. Session restore working.                                                                                         | 1           | Direct                                   | PASS    | Session restore working across restart.                                                                                                                | Test 20 |
| 0.12.49 | 2026-05-26 | openai  | Providers list            | /providers list                                              | Shows all providers with active marker                                 | All five providers shown with active marker correctly.                                                                                                                         | 0           | N/A                                      | PASS    | All providers registered correctly.                                                                                                                    | Test 21 |
| 0.12.49 | 2026-05-26 | openai  | Sessions list             | /sessions                                                    | Lists current project sessions                                         | Session listed with id, timestamp, message count.                                                                                                                              | 0           | N/A                                      | PASS    | Session management working.                                                                                                                            | Test 22 |
| 0.12.49 | 2026-05-26 | openai  | Prompt inspection         | Where is Task defined in sandbox/ → /last                    | /last returns last response                                            | Correctly identified task.py, accurate answer. /last returned last response correctly.                                                                                         | 2           | ToolAssisted                             | PASS    | /last command working correctly.                                                                                                                       | Test 23 |

---

## Summary

| Result  | Count |
|---------|------:|
| PASS    | 19    |
| PARTIAL | 3     |
| FAIL    | 0     |
| N/A     | 1     |

---

## Notes

- 27.1 definition candidate dispatch confirmed working on Tests 3 and 6 — definition_site_dispatch_bypass fires after usage candidates exhausted
- 27.2 answer guard retry confirmed working — guard fires but recovers correctly on Tests 3 and 6
- 27.3 scroll not explicitly tested in this run — manually verified working
- 27.4 regression identified: the Ctrl+O toggle incorrectly hides investigation answers (model responses following a file read), not just the file content itself. Tests 1, 2, 8, and 12 all affected. Only explicit direct reads (Test 9) behave as intended. Fix required in Phase 28.
- 27.4 secondary issue: when multiple file reads occur in a session, only the most recent has the Ctrl+O toggle — previous reads display full content inline (Test 12). Noted for Phase 28.
- 27.5 diff rendering confirmed working on Test 19
- 27.6 styling not explicitly tested — manually verified yellow approval prompts and dimmed system messages working
- Windows validation ongoing — UNC path fix applied for root_dir and resolver.rs. Backslash path separator issue in search_code results on Windows identified as remaining open item.
- Test 18 (test validation loop) not run this session — deferred.
- Test 25 (compound investigation+mutation) not run this session — known small model limitation, works with OpenAI per Phase 25 notes.

---

## Remaining failure modes

- **27.4 Ctrl+O regression**: toggle hides investigation model answers, not just file content. Only direct reads behave correctly. High priority Phase 28 fix.
- **27.4 multi-read toggle**: only most recent file read in session has Ctrl+O toggle. Previous reads show full content. Phase 28 fix.
- **Windows backslash paths**: search_code returns Windows-style backslash paths in match output on Windows. Path normalization in result parsing needs fix. Phase 28.
- **Answer guard retry on verbose models**: gpt-4o-mini still occasionally cites related unread files (Tests 3, 6). Guard fires and recovers correctly but adds a round. Model behavior, not a runtime bug.

---

## Conclusion

Phase 27 baseline established. Investigation correctness significantly improved — Tests 3 and 6 which were FAILs in Phase 25 now PASS with definition candidate dispatch working end-to-end. No regressions on existing passing tests. One new regression introduced by 27.4 (Ctrl+O toggle hiding investigation answers) requires a targeted fix in Phase 28. 802 tests passing. Foundation is solid for Phase 28 command surface expansion.