# debug-investigation

Activate when debugging retrieval failures, wrong candidates, evidence
readiness issues, non-candidate read rejections, or answer guard failures.

## When to use this skill
- Tools are firing but wrong files are being read
- Investigation terminates with InsufficientEvidence unexpectedly
- Answer guard is rejecting a seemingly correct answer
- Search budget is being exhausted too quickly
- A candidate is being selected that doesn't make sense for the mode

## Step 1 — Identify the failure type from the trace

Look for these trace events in order:
- `event=investigation_mode_detected` — confirms which mode fired
- `event=search_candidates_classified` — shows candidate counts per type
- `event=read_evidence accepted=false reason=...` — shows why a read was rejected
- `event=answer_scope_guard_rejected` — shows answer guard firing
- `event=terminal_insufficient_evidence` — shows why turn terminated

## Step 2 — Match failure to root cause

**Wrong candidate selected:**
- Check `best_candidate_for_mode()` in `src/runtime/investigation/investigation.rs`
- For DefinitionLookup: checks `first_definition_candidate()` → `definition_only_candidates` then `definition_site_candidates`
- For UsageLookup: checks `preferred_usage_candidate()` → prefers non-definition, non-import source candidates with more matches
- For General: checks first source candidate, then graph-promoted candidates

**Read rejected (accepted=false):**
- `reason=definition_lookup_non_definition_site` — file has no definition match, only usage
- `reason=candidate_read_limit_exhausted` — hit `MAX_CANDIDATE_READS_PER_INVESTIGATION = 2`
- `reason=search_candidate` with accepted=false — read was outside candidate set

**Evidence never ready:**
- Check `evidence_ready()` in `investigation.rs` — requires non-empty search AND `useful_accepted_candidate_reads >= useful_candidate_reads_target`
- Check `useful_candidate_reads_target` — broad UsageLookup raises this to 2
- Search text alone never satisfies evidence_ready

**Answer guard rejection:**
- Guard checks: (1) cited path not in `reads_this_turn`, (2) cited path outside prompt scope
- Check `engine_guards.rs` for the exact extraction logic
- No correction round is issued — terminal immediately

**Non-candidate read:**
- First offense: runtime redirects to preferred candidate if available, otherwise injects correction
- Second offense: terminal with `ReadFileFailed`
- Check `non_candidate_read_attempts` counter

## Step 3 — Key files by failure type

| Failure | Start here |
|---------|-----------|
| Wrong mode detected | `src/runtime/investigation/prompt_analysis.rs` |
| Wrong candidate | `src/runtime/investigation/investigation.rs` — `best_candidate_for_mode()` |
| Read rejected | `src/runtime/investigation/investigation.rs` — `record_read_result()` |
| Evidence never ready | `src/runtime/investigation/investigation.rs` — `evidence_ready()` |
| Answer guard | `src/runtime/orchestration/engine_guards.rs` |
| Non-candidate read | `src/runtime/orchestration/tool_round.rs` |
| Search budget | `src/runtime/orchestration/tool_round.rs` — `SearchBudget` |
| Terminal reasons | `src/runtime/types.rs` — `RuntimeTerminalReason` |

## Step 4 — Relevant tests to reference

- Non-candidate redirect: `non_candidate_read_after_search_dispatches_preferred_candidate()` in `src/runtime/tests/investigation.rs`
- Answer guard: `answer_citing_unread_path_triggers_insufficient_evidence()` in `src/runtime/tests/finalization.rs`
- Usage vs definition confusion: `usage_lookup_definition_only_reads_produce_insufficient_evidence()` in `src/runtime/tests/finalization.rs`
- Malformed syntax: `malformed_block_triggers_correction_and_retries()` in `src/runtime/tests/tool_round.rs`

## Investigation mode priority order
`CallSiteLookup` → `UsageLookup` → `ConfigLookup` → `InitializationLookup`
→ `CreateLookup` → `RegisterLookup` → `LoadLookup` → `SaveLookup`
→ `DefinitionLookup` → `General`

## Guard firing order within a turn
1. Surface enforcement (tool allowed on this surface?)
2. Mutation gate (mutation_allowed?)
3. List-before-search block
4. Read path mismatch (requested_read_path)
5. Search budget check
6. Duplicate read check
7. Non-candidate read guard
8. Candidate read cap (MAX = 2)
9. Total read cap (MAX = 3)