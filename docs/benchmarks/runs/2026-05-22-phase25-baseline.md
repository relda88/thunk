# Benchmark Run — 2026-05-22 — Phase 25 Baseline (Pre Phase 26)

Date: 2026-05-22
Version: 0.11.43
Backend: llama.cpp
Model: qwen2.5-coder-1.5b-instruct q4_k_m
Machine: M2 Air 8GB

---

## Context

Phase 25 baseline. Phases 21-25 delivered:
- 21: Session persistence & project-scoped restore
- 22: Shell tool (bounded, approval-gated, cargo allowlist)
- 23: Performance — persistent LlamaContext, incremental KV cache prefill
- 24: Observability — semantic activity labels, post-edit test validation,
  prompt inspection, evidence citations, mutation undo
- 25: Provider flexibility — .env loading, provider switching commands,
  Ollama provider, OpenRouter provider

Regression suite uses same 15 tests as Phase 20 baseline for direct
comparison. New suite covers capabilities added since Phase 20.

---

## Key Behaviors Being Measured

**Regression:**
- Investigation modes (InitializationLookup, DefinitionLookup, UsageLookup, CallSiteLookup)
- Evidence gating and guard retry convergence
- Direct reads, anchor follow-ups, simple edit seeding
- Git read-only surface
- Mutation approval pipeline

**New:**
- Shell tool approval flow and output capture
- Post-edit test validation loop
- Mutation undo/rollback
- Session restore across restart
- Provider switching (/providers list, /providers use)
- .env loading
- Prompt inspection hotkey
- Evidence citations in approval screen

---

## Results

| Version | Date       | Backend   | Scenario              | Prompt / action                    | Expected behavior             | Observed behavior                                                                 | Tool rounds | Answer mode         | Pass | Notes                                                              | Source  |
| ------- | ---------- | --------- | --------------------- | ---------------------------------- | ----------------------------- | --------------------------------------------------------------------------------- | ----------- | ------------------- | ---- | ------------------------------------------------------------------ | ------- |

---

## Summary

| Result  | Count |
| ------- | ----: |
| PASS    |     |
| PARTIAL |     |
| FAIL    |     |

---

## Notes

- Regression found during baseline: project snapshot injected on correction retry rounds confused the 1.5B model, causing read_before_answering corrections to fail. Fixed by suppressing snapshot on non-Initial/ToolResults rounds. Commit: fix(runtime): suppress project snapshot on correction retry rounds.

---

## Remaining failure modes

---

## Conclusion
