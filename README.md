# thunk

Local-first, personal AI coding assistant CLI focused on local-first workflows, modular architecture, privacy, and real coding actions.

> Version 0.36.1

---

## Overview

`thunk` is a Rust-based personal AI coding assistant built around a small, explicit runtime:

- a terminal UI for chat and control commands
- a runtime that owns conversation state and tool dispatch
- a tool layer with typed inputs and outputs
- SQLite-backed session persistence
- swappable model backends

The project is structured to keep model generation, tool execution, persistence, and UI separate instead of folding everything into one text-driven loop.

---

## Whats Different

- Runtime-owned correctness, not prompt-driven behavior
- Structural execution instead of relying on model reasoning alone
- Grounded code investigation via enforced search → read → answer flow
- Explicit tool surface constraints per turn
- Deterministic correction and terminal outcomes
- Designed to remain correct even with small or imperfect local models
- Built for local-first, low-resource environments

---

## What It Does Today

- Runs as a local terminal app with an alternate-screen TUI, collapsible transcript, role badges, command launcher, tab autocomplete, approval widget, spinner, and themed chrome.
- Supports scrollable output, collapsible tool summaries, viewport-aware collapsible focus, and expandable file reads.
- Supports multiple model backends: `llama_cpp`, `openai`, `ollama`, `openrouter`, `groq`.
- Builds a system prompt from the app name, project root, and registered tool specs.
- Bootstraps project rules from `.thunk/THUNK.md` when present (falls back to `THUNK.md` at project root for existing projects).
- Injects prompt-physics guardrails: a primacy anchor, periodic refresh, and per-turn recency field.
- Streams assistant output into the conversation while emitting UI-facing runtime events.
- Parses tool calls centrally in `src/runtime/protocol/tool_codec/`.
- Executes read-only tools immediately and pauses for approval before mutating files.
- Shows a before/after diff at mutation approval time.
- Runs an LSP pre-edit safety check for configured file extensions before approved single-file mutations.
- Can run a configurable post-mutation `project.verify_command`, then request bounded self-correction attempts on failure.
- Groups consecutive file mutations into multi-edit transactions and rolls back prior edits if one action fails.
- Re-enters model generation after tool results so the assistant can synthesize a grounded same-turn answer.
- Uses runtime-owned terminal answers when the runtime already knows the outcome, such as rejected mutations or failed file reads.
- Enforces bounded per-turn `search_code` behavior at runtime instead of relying only on prompt wording.
- Maintains a persistent SQLite-backed symbol/import index for definition and import lookup support.
- Estimates context usage, prunes stale tool results, warns at 75%, and auto-prunes at 90% context usage.
- Persists sessions in `data/sessions.db` and restores the most recent same-root session on startup.
- Writes best-effort per-session logs under `logs/`.

Current built-in tools:

- `read_file`
- `list_dir`
- `search_code`
- `edit_file`
- `write_file`
- `shell` (cargo only — requires approval)
- `git_status`
- `git_diff`
- `git_log`
- `git_branch`
- `lsp_definition` (runtime-dispatched, not registered in `ToolRegistry`)

Current control commands:

- `/help` — show available commands
- `/clear` — clear transcript history
- `/quit` — exit
- `/approve` — confirm pending mutation or shell action
- `/reject` — cancel pending action
- `/undo` — revert last mutation
- `/read <path>` — read a file directly
- `/search <query>` — search code directly
- `/ls [path]` — list a directory directly
- `/last` — show last assistant response
- `/anchors` — show current anchor state
- `/history` — show conversation history
- `/sessions` — list current project sessions
- `/session clear` — delete current project sessions and start fresh
- `/providers list` — list available providers
- `/providers use <name>` — switch active provider (session-only)
- `/git branch` — show current branch
- `/git status` — show git status
- `/git diff` — show git diff
- `/git log` — show git log
- `/lsp status` — show LSP status
- `/index build` — build the symbol/import index
- `/index build --large` — build the index without the file-count guard
- `/index status` — show symbol/import index status
- `/context stats` — show context window statistics
- `/compact` — prune stale tool results from live context
- `/prompt-physics on|off|status` — toggle prompt-physics injection for the session
- `/verify <command>|off|status` — set or inspect the post-mutation verify command
- `/transaction` — show pending transaction state

---

## Keybindings

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

Note: on macOS/crossterm, `Alt+[` may be consumed as the `ESC [` CSI prefix unless the terminal supports an extended keyboard protocol.

---

## Runtime Behavior

At a high level:

1. The user submits a prompt in the TUI.
2. The runtime sends the full in-memory conversation to the active model backend.
3. The assistant response is scanned for tool calls.
4. Tool calls are dispatched in document order.
5. Immediate tool results are injected back into the conversation as runtime-owned result blocks.
6. The runtime normally re-enters generation with those results so the model can answer from actual tool output.
7. If a mutating tool proposes a change, the runtime stores a staged `PendingAction` or grouped `PendingTransaction` and waits for `/approve` or `/reject`.

Some outcomes are deliberately terminal and runtime-owned: rejecting a pending mutation produces a cancellation answer without asking the model to summarize, and a failed `read_file` can end cleanly without retrying in a loop.

`search_code` is a literal substring search. The runtime simplifies model-generated search phrases into a single literal keyword and enforces a per-turn budget: one search is allowed, a second search is allowed only when the first returned no matches, and later search attempts are blocked with a correction so the model must answer cleanly.

Prompt physics is enabled by default. At bootstrap, `.thunk/THUNK.md` is read as a project-rule primacy anchor when present (falls back to `THUNK.md` at project root for existing projects); every generation may also receive a short refresh message and a recency field naming the current tool surface and allowed tools. `/prompt-physics` toggles this session-local injection without changing config.

Mutation approval has stages. Single-file edits can run an LSP pre-check before execution when `[lsp].enabled = true` and the file extension is listed in `[lsp].extensions`. After a successful file mutation, `project.verify_command` can run a language-agnostic verification command; failures can trigger up to `project.max_correction_attempts` corrective edit proposals. Consecutive edit/write calls are approved as a transaction and execute atomically with best-effort rollback.

---

## Execution Model

The runtime enforces a structured investigation loop rather than relying on the model to behave correctly on its own.

At a high level:

- search → read → answer gating is enforced per turn
- evidence must be established before synthesis is allowed
- tool usage is restricted by per-turn tool surfaces
- after evidence is accepted, further tool calls are blocked
- repeated violations result in runtime-owned terminal outcomes

This allows the system to remain correct and predictable even when the model makes mistakes or attempts invalid actions.

---

## Architecture

The codebase is split into seven main layers:

- `src/core/` — shared infrastructure types (AppError, Result, Config) — no dependencies on other layers
- `src/app/` — startup, config, paths, session orchestration
- `src/runtime/` — conversation loop, tool parsing, approval state, runtime events, symbol extraction, context pruning
- `src/tools/` — tool contracts, registry, and implementations
- `src/storage/` — SQLite session storage and symbol/import index storage
- `src/llm/` — backend abstraction and providers
- `src/tui/` — terminal input, rendering, and slash commands

Key architectural rules reflected in the code:

- parsing of raw tool syntax lives in `runtime/protocol/tool_codec/`
- tools operate on typed `ToolInput` / `ToolOutput`, not raw model text
- mutating tools separate `run()` from `execute_approved()` and can be wrapped in staged pending transactions
- the runtime does not depend on the TUI or SQLite directly
- the TUI renders events but does not execute tools
- all shared types (AppError, Config) are imported from `src/core/` — never from `app/`

---

## Current Limitations

- Shell allowlist is restricted to `cargo` only — broader shell access not yet supported.
- No advanced memory system.
- Summarization-based compaction is deferred; current context control uses estimation, warnings, and tool-result pruning.
- Pending approvals and transactions are not persisted across restarts.
- Restored session history is loaded into the runtime, but not replayed into the visible TUI transcript.
- No prompt caching or summarization-based context compression yet.
- Windows support is functional but ongoing — search_code path handling on Windows is an open item.

---

## Installation

Build and install to PATH:
```bash
cargo build --release
cargo install --path .
```

Without llama-cpp (Windows or faster builds):
```bash
cargo build --release --no-default-features
cargo install --path . --no-default-features
```

Once installed, run from any project directory:
```bash
cd /your/project
thunk
```

thunk walks upward from the current directory to find `config.toml` and `.git`. Copy `config.example.toml` to your project root as `config.toml` and configure your preferred provider.

---

## Running

Requirements:
- Rust stable
- Interactive terminal (`stdout` must be a TTY and `TERM` must not be `dumb`)
- A local `.gguf` model if using `llama_cpp`, or an API key for cloud providers
- `ripgrep` (`rg`) in PATH — required for `search_code`

Run during development:
```bash
cargo run --release
```

With trace logging:
```bash
# Mac/Linux
THUNK_TRACE_RUNTIME=1 cargo run --release

# Windows (cmd)
set THUNK_TRACE_RUNTIME=1
cargo run --release --no-default-features
```

Run tests:
```bash
cargo test --no-default-features
```

Configuration lives in `config.toml`. See `config.example.toml` for the sample shape. Current project-level knobs include `test_command`, `verify_command`, and `max_correction_attempts`; LSP diagnostics are guarded by `[lsp].enabled` and `[lsp].extensions`; prompt physics is controlled by `[prompt_physics].enabled`.

Provider API keys go in `.env` at the project root:
```
GROQ_API_KEY=...
OPENAI_API_KEY=...
OPENROUTER_API_KEY=...
```

Switch providers at runtime with `/providers use <name>`. Available: `llamacpp`, `openai`, `ollama`, `openrouter`, `groq`.

Recommended daily driver: Groq (`llama-3.1-8b-instant`) for cloud, Ollama (`qwen2.5-coder:7b`) for local.

---

## Documentation

| Section | Description |
| --- | --- |
| [Architecture](docs/architecture.md) | Code-accurate system architecture and runtime behavior |
| [Runtime](docs/runtime.md) | Focused overview of the runtime loop, events, and approval flow |
| [Tools](docs/tools.md) | Current tool contract, registry model, and built-in tool behavior |
| [Sessions](docs/sessions.md) | Session storage, restore behavior, and persistence limits |
| [Setup](docs/setup.md) | Requirements, run/test commands, and config basics |
| [Benchmarks](docs/benchmarks/README.md) | Performance notes and measurements |
