# /wrap-up

End-of-session summary, learnings extraction, migration proposals, and handoff snapshot. Run at the end of any implementation session. Proposes all changes — writes nothing without explicit human approval.

## Steps

**1. Summarize session**
- Run `!git log --oneline -5` and `!git diff --name-only HEAD~3..HEAD`
- Identify touched subsystems (match changed files to module-map.md ownership)
- Identify which skills were invoked this session (investigation-planner, debug-runtime, etc.)

**2. Extract learnings**
For each touched subsystem or invoked skill, propose 1-3 new entries in exact learnings.md format:
- Date | Phase | Subsystem
- Observation (one sentence, specific to thunk)
- Evidence (file path, commit, or reproduction)
- Action/Rule (actionable for future sessions)
- Impact (High/Med/Low)

Criteria — an entry is worth adding only if it is:
- Project-specific (not a generic Rust/LLM best practice)
- Evidence-based (grounded in a specific file, bug, or commit)
- Not already captured in `rules/` or `dev/`
- Actionable for future investigations or debugging

**3. Migration check**
Scan `skills/*/learnings.md` for entries tagged as High impact that appear in 2+ phases (search by subsystem keyword or cross-reference git history). An entry graduates to `rules/invariants.md` under `## Evolved Invariants` when:
- Validated across 2+ phases with no violations
- Systemic (affects multiple files or a core invariant)
- Actionable as a hard rule

Propose graduated entries with the original learnings reference. Tag the original entry [GRADUATED] rather than deleting it.

**4. Sync check**
Run `!git log --oneline -1` and check if CLAUDE.md test baseline is stale. Propose updates to:
- Test count in CLAUDE.md (run `cargo test --no-default-features 2>&1 | grep "^test result"` mentally from the last `just verify` output)
- Any `module-map.md` entries for newly added modules this session
- Any `invariants.md` line references that shifted

**5. Generate handoff snapshot**
Propose content for `dev/session-handoff-latest.md` (overwrite):

Session Handoff — [DATE] | Phase [N]

Decisions Made
- [Settled architectural decisions with evidence references]

Open Questions / Next
- [Unresolved questions, next slice to implement]

Key Files Changed
- [List from git diff]

Learnings Added
- [N entries to skills/X/learnings.md]

Invariants Graduated
- [Any entries proposed for rules/invariants.md]

Resume Prompt
"Continue Phase [N] Slice [N.X]: [one-sentence description of next action]"

**6. Output and approval**
Show all proposed changes:
- New learnings entries (with target file)
- Any graduation proposals (with target section in invariants.md)
- Handoff snapshot content
- Any sync updates (CLAUDE.md baseline, module-map.md)

Do not write any file until the human explicitly approves. State clearly: "Ready to write — approve to proceed."

## Constraints
- Evidence-based only — no generic best practices, no hallucinated file paths
- Never write without explicit human approval
- Keep learnings entries to 4-8 lines max
- Keep handoff snapshot under 200 lines — reference docs rather than duplicating them
- Do not touch any Rust source files
