# Session Handoff — 2026-06-29 | Phase 47

## Decisions Made
- Shell safety is a 3-tier classifier (`shell_tier.rs`), replacing the cargo-only allowlist: ReadOnly → shell_read (immediate); FsMutation (mkdir/cp/mv/cargo) → shell (approval, reversible:true); Exec (rm/bash/unknown/metachars) → shell (approval, reversible:false, default-deny until /exec on).
- Metachar detection precedes program classification — runtime spawns directly with no interpreter, so pipes/redirects/compound ops route to Tier-3.
- cargo is FsMutation, not Exec (writes to target/ but reversible) — corrected in 3701d62.
- Exec-gate lives in tool_round.rs (before registry.dispatch) because Tool::run() has no runtime state; deny returns TerminalAnswer + SystemMessage to avoid retry spiral.
- exec_enabled is session-scoped, default-deny, reset on /reset.
- shell_read needs an explicit PostRead answer phase (invisible to reads_this_turn).
- find -exec/-delete/-fprint* and sed -i escalate tier via flag inspection (token match, not substring).

## Open Questions / Next
- Scope Phase 48.
- Consider whether shell_read output should populate reads_this_turn uniformly rather than the per-tool answer_phase patch.
- Watch for graduation of the metachar-precedence rule if Phase 48+ re-touches shell routing.

## Key Files Changed
- New: src/runtime/investigation/shell_tier.rs, src/tools/core/shell_read.rs, src/runtime/tests/shell_exec.rs
- Reworked: src/tools/core/shell.rs, tool_round.rs, engine.rs, command_handlers.rs, tool_surface.rs, prompt_analysis.rs, prompt.rs, tool_parser.rs, tool_renderer.rs
- Wiring: types.rs, registry.rs, resolved_input.rs, resolver.rs, tui/commands/{mod,dispatch}.rs
- Docs: .claude/rules/invariants.md (Shell Allowlist section)

## Learnings Added
- 1 → investigation-planner/learnings.md (metachar precedence [High])
- 2 → debug-runtime/learnings.md (exec-gate TerminalAnswer+SystemMessage [High], shell_read PostRead admission [Med])

## Invariants Graduated
- None (single-phase arc; re-evaluate next phase).

## Resume Prompt
"Phase 47 complete (tiered shell). Test baseline now 1431. Sync CLAUDE.md phase state + baseline if not yet written, then scope Phase 48."
