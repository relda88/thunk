# Tool System

## Registration

Tool registration is split in two stages. `default_registry()` in `src/tools/mod.rs` registers only `read_file` and `list_dir`. `ToolRegistry::with_project_root()` adds `search_code`, `git_status`, `git_diff`, `git_log`, `git_branch`, `edit_file`, `write_file`, and `shell` because those tools need the runtime-owned root. `lsp_definition` is not registered in `ToolRegistry` — it is intercepted in `tool_round.rs` and dispatched directly to `LspManager`. Code: `src/tools/mod.rs`, `src/tools/registry.rs`.

`ToolRegistry` owns registration, spec lookup, dispatch, and approved execution. It does not parse assistant text, render tool results, or enforce runtime policy. Code: `src/tools/registry.rs`.

## Wire Format

The tool wire format is owned by `tool_codec` (`src/runtime/protocol/tool_codec/`). `parse_all_tool_inputs()` scans bracket calls, static Git calls, block tools, and `lsp_definition` blocks in document order; it ignores tool syntax inside Markdown code fences. `format_tool_result()` and `format_tool_error()` render the conversation-facing protocol blocks. Code: `src/runtime/protocol/tool_codec/tool_parser.rs`, `src/runtime/protocol/tool_codec/tool_renderer.rs`.

`tool_codec` accepts both canonical and tolerated drift formats. The parser handles: single-line `[read_file: ...]`, `[list_dir: ...]`, `[search_code: ...]`; block `[edit_file]...[/edit_file]`, `[write_file]...[/write_file]`, `[search_code]...[/search_code]`, `[lsp_definition]\npath: ...\nline: N\ncol: N\n[/lsp_definition]`; and fallback edit delimiters (conflict-style and labeled `old content:` / `new content:` blocks).

## Surface Exposure

Tool exposure is turn-local and surface-based:
- `RetrievalFirst`: `search_code`, `read_file`, `list_dir`, `lsp_definition`
- `GitReadOnly`: `git_status`, `git_diff`, `git_log`, `git_branch`
- `AnswerOnly`: no tools
- `MutationEnabled`: same read tools as `RetrievalFirst`; `edit_file`, `write_file`, `shell` appear in the per-turn hint extension via `mutation_tool_names()`

Surface enforcement applies only to read-only tool families. `tool_allowed_for_surface()` treats `edit_file`, `write_file`, and `shell` as outside the surface membership check because mutation permission is enforced separately. Code: `src/runtime/investigation/tool_surface.rs`, `src/runtime/orchestration/tool_round.rs`.

## Execution Kinds

Tools have two execution kinds. `ExecutionKind::Immediate` returns a `ToolOutput` in the current round. `ExecutionKind::RequiresApproval` returns a `PendingAction` and suspends the turn. Code: `src/tools/types.rs`.

## Individual Tools

- **`read_file`**: reads the target file as bytes, decodes lossily, truncates injected content at 200 lines. Code: `src/tools/read_file.rs`.
- **`list_dir`**: lists only immediate children, skips directories in `DEFAULT_SKIP_DIRS`, sorts directories before files, truncates to 200 entries. Code: `src/tools/list_dir.rs`, `src/dirs.rs`.
- **`search_code`**: shells out to `rg` (fixed-string, hidden+ignored included), limits collection to 50 matches, display to 15 matches, and 3 collected lines per file before result sorting. Code: `src/tools/search_code.rs`.
- **`git_status`**: runs `git status --short` in the project root. Code: `src/tools/git_status.rs`.
- **`git_diff`**: runs `git diff` (or `git diff <path>`) in the project root. Code: `src/tools/git_diff.rs`.
- **`git_log`**: runs `git log --oneline -20` in the project root. Code: `src/tools/git_log.rs`.
- **`git_branch`**: runs `git branch` in the project root. Added in phases 25–26. Code: `src/tools/git_branch.rs`.
- **`edit_file`**: exact-match, first-occurrence only. `run()` validates the search text exists in current file contents, returns `PendingAction`. `execute_approved()` rechecks path validity and search-text staleness before writing. Code: `src/tools/edit_file.rs`.
- **`write_file`**: proposes create or overwrite, sets risk based on current existence. `execute_approved()` refuses to create missing parent directories. Code: `src/tools/write_file.rs`.
- **`shell`**: runs an arbitrary command inside the project root with a 60-second timeout and 8 KB output cap. Only `cargo` commands are permitted (`is_permitted_shell_command()`). Always `RequiresApproval`. Code: `src/tools/shell.rs`, `src/runtime/investigation/prompt_analysis.rs`.
- **`lsp_definition`**: block-format tool. Dispatched in `tool_round.rs` before `registry.dispatch()` because `LspManager::query_definition()` requires `&mut self`. Returns the definition location of a symbol at `(path, line, col)`. On success, records a definition edge in `InvestigationGraph`. On LSP error, returns an empty `LspDefinitionOutput` — never a terminal answer. Requires `[lsp].enabled = true` in config. Code: `src/runtime/orchestration/tool_round.rs`, `src/runtime/lsp/manager.rs`, `src/core/config.rs`.

## Approval Flow

Approval flow is runtime-owned. `run_tool_round()` returns `ApprovalRequired`, `Runtime` stores the `PendingAction`, and `handle_approve()` or `handle_reject()` resolves it. Successful approval commits the tool result and ends with a runtime-authored answer; rejection injects a tool error and ends with a runtime-authored cancellation answer. Code: `src/runtime/orchestration/tool_round.rs`, `src/runtime/orchestration/engine.rs`, `src/runtime/protocol/response_text.rs`.

## Custom Commands

User-defined commands can be wired in `config.toml` under `[commands.<name>]`. Only `read_file` and `search_code` tools are permitted; `{input}` in the template is replaced with the user's argument. Parsed by `CustomCommandDef` in `src/core/config.rs`.
