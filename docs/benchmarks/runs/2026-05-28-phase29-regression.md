# Benchmark Run — 2026-05-28 — Phase 29 Regression

Date: 2026-05-28
Version: 0.14.54
Backend: openai
Model: gpt-4o-mini
Machine: MacBook Air M2, 8GB RAM

---

## Context

Post-29.14 regression run. Confirms 29.14 fix (clamp to MAX_CANDIDATE_READS_PER_INVESTIGATION=2)
is live at 862 passing. Five manual benchmark tests re-run to validate investigation behavior
across InitializationLookup, DefinitionLookup, and UsageLookup modes.

---

## Key Behaviors Being Measured

- InitializationLookup: runtime finds correct init site from truncated search results
- DefinitionLookup: runtime reads definition-site file, not call-site files
- UsageLookup: runtime reads 2 usage candidates + definition site via bypass gate
- DefinitionLookup on truncated results: runtime handles declaration in tail (matches 16–20)

---

## Results

| Version | Date | Backend | Scenario | Prompt / action | Expected behavior | Observed behavior | Tool rounds | Answer mode | Pass | Notes |
|---------|------|---------|----------|-----------------|-------------------|-------------------|-------------|-------------|------|-------|
| 0.14.54 | 2026-05-28 | openai | InitializationLookup, scoped, truncated results | Find where logging is initialized in sandbox/ | Reads logging_setup.py, answers with init site | Read z_init_target.py then logging_setup.py, correct answer | 3 | ToolAssisted | PASS | useful_target=2, broad_usage_lookup=false |
| 0.14.54 | 2026-05-28 | openai | DefinitionLookup, scoped, truncated results | Find where TaskStatus is defined in sandbox/ | Reads enums.py directly, answers with definition | Read enums.py, correct answer in 2 rounds | 2 | ToolAssisted | PASS | useful_target=1, definition selected first |
| 0.14.54 | 2026-05-28 | openai | UsageLookup, scoped, truncated results | Find where TaskStatus is used in sandbox/ | Reads 2 usage candidates + definition bypass | Read commands.py, task.py, enums.py (bypass), correct answer | 4 | ToolAssisted | PASS | useful_target=2, definition_site_dispatch_bypass fired |
| 0.14.54 | 2026-05-28 | openai | UsageLookup, scoped, no truncation | Find where TaskRepository is used in sandbox/ | Reads 2 usage candidates + definition bypass | Read test_repository.py, main.py, repository.py (bypass), correct answer | 4 | ToolAssisted | PASS | useful_target=2, call_site_files=3 |
| 0.14.54 | 2026-05-28 | openai | DefinitionLookup, no scope, truncated results | Where is run_tool_round defined? | Reads tool_round.rs, answers with fn definition | Refinement dispatch fired (fn run_tool_round), but declaration still in truncated tail — InsufficientEvidence terminal | 3 | RuntimeTerminal | PARTIAL | 29.15 refinement dispatch confirmed working (event=definition_refinement_dispatch). Fails at scale: 20 call sites across large codebase push declaration past MAX_RESULTS_SHOWN even after refinement. Phase 30 symbol index is the correct fix. |

---

## Summary

| Result | Count |
|--------|-------|
| PASS    | 4 |
| PARTIAL | 1 |
| FAIL    | 0 |
| **Total** | **5** |

---

## Known Issues

- **Test 5 (run_tool_round DefinitionLookup)**: 29.15 refinement dispatch fires correctly but cannot overcome scale — 20 call sites across a large codebase push the `fn run_tool_round` declaration past MAX_RESULTS_SHOWN even after query refinement to `fn run_tool_round`. Root fix is Phase 30 persistent symbol index. Works correctly on small/medium codebases where declaration survives truncation.
