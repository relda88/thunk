# Session Handoff — 2026-06-30 | Phase 49 (IN PROGRESS, slice 49.8)

## Decisions Made
- GUI is a second frontend over the SAME worker channel, not a parallel stack. `spawn_backend` (src/app/backend.rs) returns BackendHandle{cmd_tx, reply_rx}; both TUI and GUI drive WorkerCmd / consume WorkerReply.
- Event bridge is a pure projection: `RuntimeEventDto::try_from` drops advisory variants (BackendTiming, BackendTokenCounts, RuntimeTrace, PromptAssembled) via Err(()); `worker_reply_to_dto` is the single WorkerReply→event map (src/app/dto.rs).
- GUI slash commands delegate to crate::tui::commands (parse/resolve_command/resolve_custom_command) — command semantics single-sourced (src/gui/commands.rs).
- cwd must be captured in main() before WebKit/AppKit chdir; threaded via AppPaths::discover(start_dir_override) (a82c4c7).
- Bundled Tauri needs Vite `base: './'` and tauri.conf.json trimmed to frontendDist only — no devUrl (7b6ac6a).
- Frontend rewritten from plain HTML to React + Vite + Tailwind (b5fd8bd); all GUI Rust behind `gui` feature.

## Open Questions / Next
- Complete slice 49.8 polish (tool alignment, markdown, raw-result filtering, spinner already in 8e0e7dc); confirm remaining 49.8 scope.
- Run `just verify` and refresh the CLAUDE.md test baseline (1431 is pre-Phase-48).
- Decide dev-mode story: devUrl was removed for the bundled build — is a separate dev config needed for hot reload?
- Phase 48 shipped without a handoff entry — no blocking debt, but note for continuity.

## Key Files Changed
- New: src/gui/mod.rs, src/gui/commands.rs, src/app/dto.rs, src/app/backend.rs, tauri.conf.json, frontend/** (React/Vite/Tailwind)
- Reworked: src/app/mod.rs (run(cli, start_dir) + gui dispatch), src/app/paths.rs (discover override), src/main.rs (cwd capture), src/tui/commands/{mod,dispatch}.rs, justfile, Cargo.toml/.lock

## Learnings Added
- 2 → debug-runtime/learnings.md (cwd-before-WebKit [High], Tauri white-screen [Med])
- 2 → investigation-planner/learnings.md (DTO projection parity [Med], shared spawn_backend/command layer [Med])

## Invariants Graduated
- None (single-phase). Watch: "thread runtime-owned names into every consumer" now spans P45–46 (MCP) + P49 (DTO).

## Resume Prompt
"Continue Phase 49 slice 49.8 (Tauri GUI polish): finish frontend rendering fixes, run just verify, refresh CLAUDE.md phase state + test baseline."
