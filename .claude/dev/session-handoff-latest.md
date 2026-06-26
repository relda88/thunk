# Session Handoff — 2026-06-26 | Phase 46

## Decisions Made
- Personal memory is its own subsystem: storage in src/storage/memory/, runtime logic in src/runtime/memory/. Recall injects at session-start anchor + per-turn request-local (not persisted as history).
- Model-proposed facts are parsed via [REMEMBER: text | category] and filtered for imperatives before persistence (memory_parser.rs).
- All memory writes go through memory_write_scope; no project root is assumed (bare-dir fallback supported via AppPaths.project_label).
- MCP tool names must be threaded into all three per-turn surfaces (hint, recency, telemetry estimator). Confirmed High-impact, single chokepoint does not exist.
- Path-escape errors redirect the model to mcp::filesystem tools rather than only rejecting.

## Open Questions / Next
- Confirm Phase 46 is COMPLETE vs ACTIVE; update CLAUDE.md accordingly.
- Re-run `just verify` to capture the exact new test baseline (grep suggests ~1383 annotations vs 1330 recorded).
- Consider graduating the MCP surface-threading rule once a future phase re-touches tool registration (2+ phase bar).

## Key Files Changed
- New: src/runtime/memory/{manager,mod}.rs, src/storage/memory/{schema,store,types,mod}.rs, src/runtime/protocol/memory_parser.rs, src/runtime/orchestration/memory_handlers.rs
- MCP polish: generation.rs, context_cap.rs, prompt_physics.rs, engine.rs, project/resolver.rs, tools/core/list_dir.rs
- TUI: app.rs, events.rs, input.rs, state.rs, renderer/mod.rs, commands/{mod,dispatch}.rs
- Tests: runtime/tests/{memory_command,reflect_command,index_embed}.rs

## Learnings Added
- 4 → investigation-planner/learnings.md (MCP three-surface threading [High], imperative filter [Med], escape redirect [Med], memory write scope [Med])
- 1 → debug-runtime/learnings.md (MCP call no-op symptom→cause [High])

## Invariants Graduated
- None this cycle (MCP entries confined to a single phase arc).

## Resume Prompt
"Phase 46 complete (personal memory + MCP polish). Run `just verify`, update CLAUDE.md phase state + test baseline, then scope Phase 47."
