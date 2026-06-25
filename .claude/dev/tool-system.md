# Tool System

## Registration

Tool registration is split in two stages. `default_registry()` in `src/tools/mod.rs` registers only `read_file` and `list_dir`. `ToolRegistry::with_project_root()` adds `search_code`, `git_status`, `git_diff`, `git_log`, `git_branch`, `edit_file`, `write_file`, and `shell` because those tools need the runtime-owned root. `lsp_definition` is not registered in `ToolRegistry` — it is intercepted in `tool_round.rs` and dispatched directly to `LspManager`. Code: `src/tools/mod.rs`, `src/tools/registry.rs`.

`ToolRegistry` owns registration, spec lookup, dispatch, and approved execution. It does not parse assistant text, render tool results, or enforce runtime policy. Code: `src/tools/registry.rs`.

The built-in tool set is no longer purely compile-time. `ToolRegistry` keys tools by `String` (not `&'static str`), and MCP tools are registered dynamically at runtime (Phase 45). Dynamic calls arrive as `ToolInput::DynamicTool { name, args }` with metadata in `DynamicToolSpec`; the surface policy admits them via the runtime-held `dynamic_allowed` set in `tool_allowed_for_surface()` rather than the static `TOOL_SURFACE_DEFINITIONS`. `MCPManager` (`src/runtime/mcp/`) owns the server lifecycle that provides these tools. Code: `src/tools/types.rs`, `src/runtime/investigation/tool_surface.rs`, `src/runtime/mcp/manager.rs`.

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

Tools have two execution kinds. `ExecutionKind::Immediate` returns a `ToolOutput` in the current round. `ExecutionKind::RequiresApproval` returns a `PendingAction` and suspends the turn. The runtime stores approvals as `PendingApprovalStage`; consecutive edit/write approvals can be collected into a `PendingTransaction`. Code: `src/tools/types.rs`, `src/tools/pending.rs`.

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
- **`lsp_definition`**: block-format tool. Dispatched in `tool_round.rs` before `registry.dispatch()` because `LspManager::query_definition()` requires `&mut self`. Returns the definition location of a symbol at `(path, line, col)`. On success, records a definition edge in `InvestigationGraph`. On LSP error, returns an empty `LspDefinitionOutput` — never a terminal answer. Requires `[lsp].enabled = true` in config; diagnostics/pre-checks only run for `[lsp].extensions`. Code: `src/runtime/orchestration/tool_round.rs`, `src/runtime/lsp/manager.rs`, `src/core/config.rs`.
- **`web_fetch`**: `ExecutionKind::Immediate`. Fetches a URL over HTTPS, strips HTML tags, truncates at 32 KB (`MAX_FETCH_BYTES`), and blocks private/loopback IP addresses. Not registered in `ToolRegistry` — dispatched directly by `handle_fetch_url()` in `command_handlers.rs`. `/fetch <url>` maps to `RuntimeRequest::FetchUrl { url }`. Code: `src/tools/core/web_fetch.rs`, `src/runtime/orchestration/command_handlers.rs`.

## Approval Flow

Approval flow is runtime-owned. `run_tool_round()` returns `ApprovalRequired` for one action or `TransactionRequired` for consecutive edit/write actions. `Runtime` stores the pending work as `PendingApprovalStage::AwaitingPreCheck`; single-file mutations can advance to `PreCheckComplete` after an LSP pre-edit safety check. Successful approval commits tool results and ends with a runtime-authored answer; rejection injects tool errors and ends with a runtime-authored cancellation answer. Code: `src/runtime/orchestration/tool_round.rs`, `src/runtime/orchestration/engine.rs`, `src/tools/pending.rs`, `src/runtime/protocol/response_text.rs`.

## Verification Flow

After approved `edit_file` or `write_file`, the runtime may run `project.verify_command` directly from `execute_and_handle()`. This command is language-agnostic and not routed through `ShellTool` because it is a runtime verification step. On failure, the runtime can inject a `[runtime:correction]` prompt and request another approved edit until `project.max_correction_attempts` is reached. Transactions run `verify_command` after all edits but skip self-correction. `/verify <command>|off|status` changes the session-scoped command. Code: `src/runtime/orchestration/engine.rs`, `src/runtime/orchestration/command_handlers.rs`, `src/core/config.rs`.

## Prompt Physics

Prompt physics is owned by `src/runtime/protocol/prompt_physics.rs`. `THUNK.md` is loaded at app bootstrap for the primacy anchor, `run_generate_turn()` appends refresh and recency messages per generation, and `/prompt-physics on|off|status` toggles the session-local config. None of these injected messages are persisted into conversation history. Code: `src/app/mod.rs`, `src/runtime/orchestration/generation.rs`, `src/runtime/protocol/prompt.rs`, `src/runtime/protocol/prompt_physics.rs`.

## Custom Commands

User-defined commands can be wired in `config.toml` under `[commands.<name>]`. Only `read_file` and `search_code` tools are permitted; `{input}` in the template is replaced with the user's argument. Parsed by `CustomCommandDef` in `src/core/config.rs`.

## RuntimeRequest Variants Added in Phase 37–38

These variants are not backed by `ToolRegistry` tools — they are handled inline in `engine.rs` or delegated to handler methods extracted into separate files.

- **`FetchUrl { url }`** → `handle_fetch_url()` in `command_handlers.rs`. Source: `/fetch <url>`.
- **`PlanCreate { goal }`**, **`PlanApprove`**, **`PlanAbandon`**, **`PlanStatus`** → `handle_plan_create/approve/abandon/status()` in `plan_handlers.rs`. Source: `/plan <goal>|approve|abandon|status`.
- **`TaskExecute { id }`**, **`TaskComplete { id, summary }`**, **`TaskBlock { id, reason }`**, **`TaskStatus`** → `handle_task_execute/complete/block/status()` in `plan_handlers.rs`. Source: `/task execute|complete|block|status`.
- **`IndexEmbed`** → `handle_index_embed()` in `embed_handlers.rs`. Starts a chunked embed loop over top-2000 symbols. Source: `/index embed`.
- **`IndexEmbedChunk`** → `handle_index_embed_chunk()` in `embed_handlers.rs`. Internal only — never constructed from TUI code. Drives the chunked embed loop; state owned by `Runtime::pending_embed` (`PendingEmbedState`).
- **`InvestigationDepthToggle { depth }`** → `handle_depth_toggle()` in `command_handlers.rs`. Source: `/depth shallow|normal|deep`.
- **`RetrievalLog { n }`** → `handle_retrieval_log()` in `command_handlers.rs`. Source: `/retrieval log [n]`.
