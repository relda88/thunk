# Layer Architecture

## Dependency Order (bottom → top)
core/ → tools/ → runtime/ → app/ → tui/

## Rules
- Always import AppError, Result, Config from crate::core — never from crate::app
- app/config.rs and app/error.rs are thin re-exports only
- tui/ contains no business logic — rendering and event dispatch only
- Lower layers never import from higher layers

## What src/core/ Exports
- AppError, Result (error.rs)
- Config, GroqConfig, OllamaConfig, and all sub-configs + load() (config.rs)

## Known Exception
src/core/error.rs imports ToolError from src/tools/ for the From<ToolError> for AppError impl.
This is the only place the "core has no outward deps" invariant is broken.
Tracked as tech debt — fix is to move the From impl to app/ or a runtime conversion module.

## Intentional Bidirectional Dependency
tools/ imports ResolvedToolInput, ProjectPath, ProjectScope, ProjectRoot from runtime/project/.
This is intentional — runtime/project/ owns the path confinement types that tools need.
tools/ sits above runtime/project/ but below runtime/orchestration/.

## TUI Layer Rule
TUI events flow: RuntimeEvent → apply_runtime_event() → state mutations only.
No business logic in tui/. No tool dispatch from tui/. No direct runtime calls except via RuntimeRequest.
