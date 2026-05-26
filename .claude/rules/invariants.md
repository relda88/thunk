# Enforced Invariants

## Mutation Approval Gate
ShellTool, EditFileTool, WriteFileTool always return ToolRunResult::Approval(PendingAction).
The only materialization path is ToolRegistry::execute_approved() in src/tools/registry.rs.
There is no bypass. Never add one.

## Shell Allowlist
is_permitted_shell_command() at src/runtime/investigation/prompt_analysis.rs — matches only "cargo".
Enforced twice in TurnContext::build() in engine.rs: as an error gate (line ~1492) and in seed_pending_runtime_call() (line ~1526).
Shell seeding is suppressed entirely on GitReadOnly turns.

## Surface Enforcement
tool_allowed_for_surface() at src/runtime/investigation/tool_surface.rs.
Surfaces and tool sets defined in TOOL_SURFACE_DEFINITIONS (static registry).
Mutation tools return None from SurfaceTool::from_input() — they bypass surface enforcement and go through approval only.

## Evidence Gates
Eight named gates in InvestigationState::record_read_result() in investigation.rs.
evidence_ready() at investigation.rs:612 — requires search_produced_results && useful_accepted_candidate_reads >= target.
Gates are never weakened. Never add a bypass.

## System Prompt
Always built fresh via build_system_prompt() from config — never persisted to SQLite.
Always called with include_mutation_tools: false (engine.rs:106).
Mutation tools appear only in the ephemeral per-turn hint for MutationEnabled turns.

## Session Scoping
All tool inputs confined via resolve() in src/runtime/project/resolver.rs.
ProjectRoot::new() canonicalizes and validates at construction.
