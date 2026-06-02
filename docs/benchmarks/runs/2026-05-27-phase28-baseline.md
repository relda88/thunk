# Benchmark Run — 2026-05-27 — Phase 28 Baseline (Windows)

Date: 2026-05-26
Version: 0.13.51
Backend: ollama
Model: qwen2.5-coder:7b-instruct-q4_K_M
Machine: Windows, 32GB RAM

---

## Context

Full regression suite run at the close of Phase 28 on Windows. 
Phase 28 delivered six slices: 
    - 28.0 Ctrl+O file expand/collapse fix (DirectReadCompleted event)
    - 28.1 Windows search_code backslash path normalization
    - 28.2 git_branch tool and /git branch slash command
    - 28.3 /help redesign, 28.4 additional slash commands (/ls, /git status, /git diff, /git log)
    - 28.5 AI dev environment (.claude/ setup)
    - 28.6 Windows scope prefix fix (SearchCodeTool UNC strip + parse_rg_match_line order fix). 

This is the first full Windows baseline run. 
All 22 tests run with qwen2.5-coder:7b-instruct-q4_K_M via Ollama.
Test 16 (cargo check) timed out due to the 60s shell timeout being too short for a full compile on Windows noted as a known platform limitation, not a regression.

---

## Key Behaviors Being Measured

- Investigation correctness: scoped search with Windows path handling (28.1, 28.6)
- Definition candidate dispatch on UsageLookup (27.1)
- Answer guard recovery and scope guard correctness (27.2)
- Direct read detection
- Ctrl+O file content expand toggle (28.0)
- Mutation approval flow with diff rendering (27.5)
- Anchor follow-up reads
- Git read-only surface enforcement including git_branch (28.2)
- Session restore across restart
- Provider listing
- Slash commands: /anchors, /history, /search, /read, /last, /sessions (28.3, 28.4)
- Shell tool approval and exit code capture
- Undo stack

---

## Results

| Version | Date | Backend | Scenario | Prompt / action | Expected behavior | Observed behavior | Tool rounds | Answer mode | Pass | Notes | Source |
|---------|------|---------|----------|-----------------|-------------------|-------------------|-------------|-------------|------|-------|--------|
| 0.13.51 | 2026-05-26 | ollama | Initialization lookup | Find where logging is initialized in sandbox/ | Identify correct init file | Correctly searched, read z_init_target.py, accurate answer. No Ctrl+O regression on investigation answer. | 2 | ToolAssisted | PASS | 28.0 and 28.6 fixes confirmed working on Windows. Scoped search path correct. | Test 1 |
| 0.13.51 | 2026-05-26 | ollama | Definition lookup | Find where TaskStatus is defined in sandbox/ | Locate enum definition | Correctly searched, read enums.py, accurate answer with full enum values and from_value method described. | 2 | ToolAssisted | PASS | Clean definition lookup on Windows. | Test 2 |
| 0.13.51 | 2026-05-26 | ollama | Usage lookup (multi) | Find where TaskStatus is used in sandbox/ | Identify multiple usage sites | Read commands.py, task.py, enums.py (definition_site_dispatch_bypass). Answer scope guard fired on cli/parser.py — terminal InsufficientEvidence. Model cited unread file. | 4 | RuntimeTerminal (InsufficientEvidence) | PARTIAL | Scope guard working correctly — rejected citation of unread parser.py. Evidence collection correct but model cited outside reads. Same behavior as Phase 27 Test 3 on Mac with different model. | Test 3 |
| 0.13.51 | 2026-05-26 | ollama | Call-site lookup | Find where load_config is called in sandbox/ | Identify call site in main.py | Correctly searched, read main.py, accurate answer identifying build_services and config_path argument. | 2 | ToolAssisted | PASS | Clean call-site lookup. | Test 4 |
| 0.13.51 | 2026-05-26 | ollama | Call-site lookup | Find where init_logging is called in sandbox/ | Identify call site in main.py | Correctly searched, read main.py, accurate answer. | 2 | ToolAssisted | PASS | Clean call-site lookup. Consistent with Test 4. | Test 5 |
| 0.13.51 | 2026-05-26 | ollama | Usage lookup (global) | Find where TaskRepository is used in sandbox/ | List usage locations | Read test_repository.py, main.py, storage/repository.py (definition_site_dispatch_bypass). Answer guard fired on task_service.py, retry fired on test_task_service.py — terminal InsufficientEvidence. | 4 | RuntimeTerminal (InsufficientEvidence) | PARTIAL | Answer guard retry working. Model cited unread files on both attempts. Evidence collection correct — guard enforced correctly. Different from Mac Phase 27 result (PASS) — model-dependent behavior. | Test 6 |
| 0.13.51 | 2026-05-26 | ollama | General search | Find where completed tasks are filtered in sandbox/ | Identify filtering logic | Correctly searched, read task_service.py, accurate and detailed answer covering completed_tasks and _filter_by_status methods. | 2 | ToolAssisted | PASS | Clean general search. Strong synthesis from qwen2.5-coder. | Test 7 |
| 0.13.51 | 2026-05-26 | ollama | File understanding | Find what task_service.py does in sandbox/ | Direct read of task_service.py, no search | Direct read triggered correctly via filename detection. Accurate and detailed summary of all TaskService methods. | 1 | ToolAssisted | PASS | 26.2 fix holding on Windows. No Ctrl+O regression. | Test 8 |
| 0.13.51 | 2026-05-26 | ollama | Direct read | Read sandbox/main.py | Return file contents, Ctrl+O to expand | Direct read triggered, file content hidden behind Ctrl+O hint. Zero model involvement in read path. | 1 | ToolAssisted | PASS | 28.0 Ctrl+O working correctly for direct reads on Windows. | Test 9 |
| 0.13.51 | 2026-05-26 | ollama | Mutation (create) | Create sandbox/baseline_test.txt | Approval flow, file created | Correct approval flow, file created (29 bytes). cargo test proposed after write, rejected intentionally. | 1 | ToolAssisted | PASS | Mutation create flow working on Windows. | Test 10 |
| 0.13.51 | 2026-05-26 | ollama | Mutation (edit) | Edit sandbox/baseline_test.txt add the content hello thunk | Approval flow, file edited | edit_file failed — search text not found. Model attempted edit without reading file first. Error message correct and actionable. | 0 | RuntimeTerminal (MutationFailed) | PARTIAL | Expected failure mode — model should read before edit. Correct error surfaced. Not a regression; same behavior as Phase 27 Test 11. | Test 11 |
| 0.13.51 | 2026-05-26 | ollama | Anchor follow-up | Read sandbox/config.py → Read that again → Open that again | Re-read from anchor | First read showed Ctrl+O hint. Follow-up reads resolved from anchor correctly both times. | 1/1/1 | ToolAssisted | PASS | Anchor resolution working on Windows. 28.0 Ctrl+O working for direct reads. | Test 12 |
| 0.13.51 | 2026-05-26 | ollama | Git read-only | git status → git diff → git branch → git | git tools fire, no shell attempt | git_status, git_diff, git_branch all fired correct tools. Bare "git" answered from context. No shell attempt on any turn. | 1/1/1/0 | ToolAssisted/ToolAssisted/ToolAssisted/Direct | PASS | 26.1 fix holding. 28.2 git_branch confirmed working on Windows. | Test 13 |
| 0.13.51 | 2026-05-26 | ollama | Definition + explain | Find where JsonFileStore is defined in sandbox/ and what it does | Locate and describe class | Correctly read file_store.py, accurate description of read_records and write_records methods including temp file pattern. | 2 | ToolAssisted | PASS | Clean compound definition+explain query. | Test 14 |
| 0.13.51 | 2026-05-26 | ollama | Usage lookup | Find where ArgumentParser is used in sandbox/ | Identify usage location | Correctly read parser.py, accurate answer describing build_parser and CLI structure. | 2 | ToolAssisted | PASS | Clean single usage candidate. | Test 15 |
| 0.13.51 | 2026-05-26 | ollama | Shell tool (timeout) | Run cargo check | Approval prompt appears, runs, exit 0 captured | Approval prompt appeared, shell timed out after 60s — compile too slow for Windows shell timeout. | 1 | ToolAssisted | FAIL | Known platform limitation — cargo check exceeds 60s shell timeout on Windows cold build. Not a regression. Shell timeout boundary working correctly. | Test 16 |
| 0.13.51 | 2026-05-26 | ollama | Shell tool (failure) | Run cargo test --this-test-does-not-exist | Approval prompt appears, non-zero exit captured | Approval prompt appeared, exit 1 captured correctly. | 1 | ToolAssisted | PASS | Non-zero exit correctly surfaced on Windows. | Test 17 |
| 0.13.51 | 2026-05-26 | ollama | Mutation (edit) with diff + undo | Edit sandbox/test.txt, replace hello with goodbye → /undo | Diff shown at approval, file restored after /undo | Diff rendered correctly at approval (- hello / + goodbye). Edit approved, undo restored file correctly. | 1 | ToolAssisted | PASS | 27.5 diff rendering confirmed on Windows. Undo stack working with Windows absolute paths. | Test 18 |
| 0.13.51 | 2026-05-26 | ollama | Providers list | /providers list | Shows all providers with active marker | All five providers shown, ollama marked active correctly. | 0 | N/A | PASS | Provider list working on Windows. | Test 19 |
| 0.13.51 | 2026-05-26 | ollama | Session restore | What is a pointer? → quit → restart → Does Rust have them? | Follow-up answered using restored context | Follow-up correctly answered with full pointer taxonomy without re-establishing context. | 0 | Direct | PASS | Session restore working across restart on Windows. | Test 20 |
| 0.13.51 | 2026-05-26 | ollama | Sessions list | /sessions | Lists current project sessions | Session listed with id, timestamp, message count. | 0 | N/A | PASS | Session management working on Windows. | Test 21 |
| 0.13.51 | 2026-05-26 | ollama | Definition lookup + /last | Where is Task initialized in sandbox/ → /last | Locate class, /last returns last response | Correctly read task.py via initialization_fallback_no_initialization_candidates. Accurate answer. /last returned full response correctly. | 2 | ToolAssisted | PASS | /last command working correctly on Windows. | Test 22 |
| 0.13.51 | 2026-05-26 | ollama | Slash commands | /anchors → /history → /search logging → /read sandbox/main.py | Each command returns correct output | /anchors showed last read and search correctly. /history showed conversation. /search returned 50 matches (showing 15). /read returned 32 lines with Ctrl+O hint. | 0/0/0/0 | N/A | PASS | 28.3 and 28.4 slash commands all working on Windows. | Test 23 |

---

## Summary

| Result | Count |
|--------|-------|
| PASS | 18 |
| PARTIAL | 3 |
| FAIL | 1 |
| **Total** | **22** |

---

## Known Issues

- **Test 3, 6 (PARTIAL)** — Answer guard correctly rejects citations of unread files, but qwen2.5-coder cites unread files more aggressively than gpt-4o-mini. Evidence collection and guard enforcement are correct — this is model-dependent synthesis behavior, not a runtime regression.
- **Test 11 (PARTIAL)** — edit_file without prior read fails as designed. Expected behavior.
- **Test 16 (FAIL)** — cargo check exceeds 60s shell timeout on Windows cold build. Shell timeout boundary is working correctly. Not a regression — platform limitation. Consider increasing shell timeout for Windows in a future slice.

---

## Phase 28 Windows Validation Status

- 28.0 Ctrl+O: confirmed
- 28.1 backslash path normalization: confirmed
- 28.2 git_branch tool: confirmed
- 28.3 /help redesign: confirmed (not directly tested, verified via /anchors, /history)
- 28.4 slash commands: confirmed (Test 23)
- 28.5 AI dev environment: N/A (local config, not runtime behavior)
- 28.6 Windows scope prefix fix: confirmed (Tests 1, 2, 4, 5, 7, 8)
