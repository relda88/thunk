# /sync-claude

Audit the current state of `.claude/` and `CLAUDE.md` against the actual codebase and update anything stale. This command keeps the AI development environment in sync with reality.

## What to check and update

**1. Test baseline in `CLAUDE.md`**
Run `cargo test --no-default-features 2>&1 | grep "^test result"` and update the test count in CLAUDE.md if it has changed.

**2. Invariant locations in `.claude/rules/invariants.md`**
Verify these line number references are still accurate:
- `is_permitted_shell_command()` in `src/runtime/investigation/prompt_analysis.rs`
- `execute_approved()` in `src/tools/registry.rs`
- `evidence_ready()` in `src/runtime/investigation/investigation.rs`
- `tool_allowed_for_surface()` in `src/runtime/investigation/tool_surface.rs`
Update any stale line references.

**3. Layer boundaries in `.claude/rules/architecture.md`**
Check if the known `core/ → tools/` violation still exists:
`grep -n "ToolError" src/core/error.rs`
If it's been fixed, remove the "Known Exception" section. If new violations exist, document them.

**4. Test command accuracy**
Verify `just verify` still runs `cargo test --no-default-features`:
`grep "test" justfile`
Update CLAUDE.md or slice-discipline.md if the command has changed.

**5. New tools or surfaces**
Check if new tools have been added since last sync:
`ls src/tools/`
If new tools exist that aren't documented in `rules/invariants.md` (under Surface Enforcement), add them.

**6. Key files table in `CLAUDE.md`**
Verify all referenced files still exist at the listed paths:
`find src -name "*.rs" | grep -E "registry|prompt_analysis|tool_surface|investigation|prompt|engine|tool_round"`
Update any moved or renamed files.

**7. Phase references**
Check the current phase from recent git log:
`git log --oneline -5`
If CLAUDE.md or any rules file references a stale phase number, update it.

## After auditing
Report what was checked, what was stale, and what was updated. Do not touch any Rust source files. Do not run `cargo test` — use the grep/find commands above for verification only.

