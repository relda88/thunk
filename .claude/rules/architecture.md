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
- Config and all sub-configs + load() (config.rs), including `ProjectConfig`, `LspConfig`, provider configs, custom commands, and `PromptPhysicsSettings`

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

## TUI Module Structure
- `mod.rs` owns terminal setup/teardown and module declarations.
- `app.rs` owns the event loop, worker reply handling, and render scheduling.
- `worker.rs` owns the background `AppContext` command runner.
- `cursor.rs` owns terminal cursor affordance sync.
- `keybindings.rs` owns key event dispatch.
- `events.rs` maps `RuntimeEvent` to `AppState`.
- `format.rs` owns UI formatting helpers.
- `state.rs` owns mutable UI state.
- `input.rs` owns input editing, history, reverse search, launcher, and autocomplete state transitions.
- `collapsible.rs` owns collapsible classification as a pure function with no renderer dependency.
- `commands/mod.rs` owns slash command parsing, autocomplete names, and launcher entries.
- `commands/dispatch.rs` maps parsed commands to worker/runtime requests.
- `renderer/mod.rs` owns `Renderer`, transcript painting, overlays, approval widget, spinner, and themed chrome.
- `renderer/buffer.rs`, `renderer/diff.rs`, `renderer/style.rs`, and `renderer/symbols.rs` own frame storage, diff output, `Theme`/packed style, and symbol interning.

`Theme` is wired into `Renderer` through `renderer/style.rs`; it is not a standalone architectural concern outside the renderer.
