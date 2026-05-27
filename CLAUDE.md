# thunk

Local-first AI coding assistant CLI in Rust. Runtime owns all control flow — model is a stateless text emitter only. Long-term goal: replace Claude Code/Codex with a private self-hosted tool optimized for consumer hardware.

## Hard Stop
Before any commit: `just verify` (fmt --check + check + clippy + test)
Test baseline: 844 passing via `cargo test --no-default-features`
Never make commits — user commits manually.

## Core Principles
- Runtime is the single source of correctness — not the model
- Backend is a stateless text emitter only
- Tools are pure execution units with approval gating
- All reasoning constraints enforced in runtime, not prompt
- Evidence-first retrieval before answer admission
- No text-as-API between subsystems
- Lower layers never depend on higher layers

## Non-Negotiable Invariants
- Mutations require explicit approval — PendingAction → execute_approved() only
- Evidence gates are never weakened
- System prompt never persisted — always rebuilt from config on restore
- Shell allowlist: cargo only
- Mutation tools excluded from system prompt on RetrievalFirst and GitReadOnly surfaces
- Provider switching is session-only
- All shared types imported from src/core/ — never from app/

## Key Files
| Task | File |
|------|------|
| Mutation approval gate | src/tools/registry.rs |
| Shell allowlist | src/runtime/investigation/prompt_analysis.rs |
| Surface enforcement | src/runtime/investigation/tool_surface.rs |
| Evidence gates | src/runtime/investigation/investigation.rs |
| System prompt | src/runtime/protocol/prompt.rs |
| Turn loop | src/runtime/orchestration/engine.rs |
| Tool dispatch | src/runtime/orchestration/tool_round.rs |
| Shared types | src/core/ |

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