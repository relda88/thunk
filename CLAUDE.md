# thunk

Local-first AI coding assistant CLI in Rust. Runtime owns all control flow — model is a stateless text emitter only. Long-term goal: replace Claude Code/Codex with a private self-hosted tool optimized for consumer hardware.

## Hard Stop
Before any commit: `just verify` (fmt --check + check + clippy + test)
Test baseline: 1030 passing via `just verify`
Never make commits — user commits manually.

## Current Phase State
- Phase 29: COMPLETE
- Phase 30: COMPLETE — persistent symbol/import index backed by SQLite
- Phase 31: COMPLETE — context window intelligence; Slice 31.5 summarization deferred
- Phase 32: COMPLETE — TUI overhaul
- Phase 33: COMPLETE — prompt physics, THUNK.md bootstrap, `/prompt-physics`
- Phase 34: COMPLETE — staged approvals, verification, correction loop, transactions

## Core Principles
- Runtime is the single source of correctness — not the model
- Backend is a stateless text emitter only
- Tools are pure execution units with approval gating
- All reasoning constraints enforced in runtime, not prompt
- Evidence-first retrieval before answer admission
- No text-as-API between subsystems
- Lower layers never depend on higher layers

## Non-Negotiable Invariants
- Mutations require explicit approval — PendingAction/PendingTransaction → execute_approved() only
- Evidence gates are never weakened
- System prompt never persisted — always rebuilt from config on restore
- Shell allowlist: cargo only
- Mutation tools excluded from system prompt on RetrievalFirst and GitReadOnly surfaces
- Provider switching is session-only
- All shared types imported from src/core/ — never from app/
- Prompt physics is request-local/session-scoped; THUNK.md may anchor prompts but is never persisted as conversation state
- Post-mutation verification is runtime-initiated via configurable `project.verify_command`

## Key Files
| Task | File |
|------|------|
| Mutation approval gate | src/tools/registry.rs |
| Shell allowlist | src/runtime/investigation/prompt_analysis.rs |
| Surface enforcement | src/runtime/investigation/tool_surface.rs |
| Evidence gates | src/runtime/investigation/investigation.rs |
| System prompt | src/runtime/protocol/prompt.rs |
| Prompt physics | src/runtime/protocol/prompt_physics.rs |
| Approval stages / transactions | src/tools/pending.rs |
| Turn loop | src/runtime/orchestration/engine.rs |
| Tool dispatch | src/runtime/orchestration/tool_round.rs |
| Runtime config knobs | src/core/config.rs |
| Approval rendering | src/tui/renderer/mod.rs |
| Shared types | src/core/ |

## TUI Module Structure
- `src/tui/mod.rs` — terminal setup/teardown and module declarations
- `src/tui/app.rs` — TUI event loop, worker channel integration, render scheduling
- `src/tui/worker.rs` — background `AppContext` command runner
- `src/tui/cursor.rs` — terminal cursor shape/affordance sync
- `src/tui/keybindings.rs` — key event dispatch
- `src/tui/events.rs` — `RuntimeEvent` to `AppState` mapping
- `src/tui/format.rs` — UI formatting helpers
- `src/tui/state.rs` — mutable UI state
- `src/tui/input.rs` — input buffer, history, reverse search, launcher, autocomplete
- `src/tui/collapsible.rs` — pure collapsible summary classification
- `src/tui/commands/mod.rs` — slash command parsing, autocomplete names, launcher entries
- `src/tui/commands/dispatch.rs` — command to `RuntimeRequest`/worker dispatch
- `src/tui/renderer/mod.rs` — renderer, transcript painting, overlays, spinner, approval widget
- `src/tui/renderer/buffer.rs` — cell buffer
- `src/tui/renderer/diff.rs` — frame diff writer
- `src/tui/renderer/style.rs` — `Theme`, colors, packed styles
- `src/tui/renderer/symbols.rs` — symbol interning

Note: `src/tui/renderer/transcript.rs` is not present in the current tree; transcript rendering lives in `renderer/mod.rs`.

## TUI Keybindings
| Key | Behavior |
| --- | --- |
| `Ctrl+C`, `Ctrl+Q` | Quit |
| `Enter` | Submit input, accept launcher, or accept reverse search depending on active mode |
| `Alt+Enter` | Insert newline |
| `Backspace` | Delete before cursor, launcher query char, or reverse-search query char depending on active mode |
| `Alt+Backspace`, `Ctrl+W` | Delete word before cursor |
| `Left`, `Right` | Move cursor |
| `Home`, `End` | Move to current logical line start/end |
| `Ctrl+D` | Dump last assembled prompt to temp file |
| `Ctrl+P` | Recall previous input |
| `Ctrl+N` | Reject pending approval, otherwise recall next input |
| `Ctrl+Y` | Approve pending approval |
| `Up`, `Down` | Cycle launcher selection when launcher is active; otherwise scroll transcript by 1 |
| `PageUp`, `PageDown` | Scroll transcript by 10 |
| `Ctrl+O` | Toggle expanded file-read transcript view |
| `Ctrl+K` | Open command launcher when not busy |
| `Ctrl+R` | Start/cycle reverse search |
| `Esc` | Cancel launcher, autocomplete, or reverse search depending on active mode |
| `Tab` | Forward slash-command autocomplete when not busy |
| `Shift+Tab` / `BackTab` | Reverse slash-command autocomplete when not busy |
| `Alt+[` | Focus previous collapsible block where supported by terminal protocol |
| `Alt+]` | Focus next collapsible block |
| `Alt+O` | Toggle focused collapsible block |
| Printable characters | Insert into input, launcher query, or reverse-search query depending on active mode |

## Build
```bash
cargo check --all-targets                                    # fast type-check
cargo test --no-default-features                             # run all tests
cargo build --release --no-default-features                  # build
just verify                                                  # full pre-commit gate
THUNK_TRACE_RUNTIME=1 cargo run --release --no-default-features  # debug
```

## Anti-Patterns — Never Reintroduce
- Parsing assistant text outside tool_codec
- UI-driven execution logic
- Weakening evidence gates
- Model involvement in structural decisions
- Importing AppError or Config from app/ — use core/
- Treating `Theme` as a standalone TUI concern outside `Renderer`

## Reference Docs
@.claude/rules/invariants.md
@.claude/rules/architecture.md
@.claude/rules/slice-discipline.md
@.claude/rules/safe-modification.md

## On-Demand Reference — Load Only When Relevant
- `.claude/dev/module-map.md` — module ownership and file locations. Read when adding new modules, tracing ownership boundaries, or unsure where a type lives.
- `.claude/dev/core-loop.md` — runtime loop internals. Read when modifying `engine.rs` or orchestration.
- `.claude/dev/tool-system.md` — tool inventory and wiring. Read when adding or modifying tools.
- `.claude/skills/debug-investigation/` — investigation, guards, failure modes. Read when modifying investigation or candidate selection.
- `.claude/skills/debug-runtime/` — debugging entry points. Read when diagnosing runtime failures.
- `.claude/skills/investigation-planner/SKILL.md` — evidence-first exploration before any implementation. Read before writing any implementation prompt.
