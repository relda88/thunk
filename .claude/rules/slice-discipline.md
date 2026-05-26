# Slice Implementation Discipline

## The Pattern (follow exactly)
1. Identify the exact failure mode — repro or failing test first
2. Find the runtime location that owns the decision — grep before assuming
3. Make the minimal change — guard condition, terminal answer, or detection pattern
4. Add a test that would have caught the regression
5. Run just verify — this is the hard stop, 818 tests must pass
6. Report to user — never commit, user commits manually

## Where Changes Live
- Behavioral changes: runtime/ or investigation/ only
- TUI changes: tui/ only, no business logic
- New tool: tools/ + wire through types.rs, registry.rs, tool_surface.rs, tool_parser.rs, tool_renderer.rs
- Never add correction logic outside runtime/ and tool_codec/
- Parsing belongs only in tool_codec/ — tools never parse raw model text

## InvestigationState Rules
- New state fields must reset in new() (the large initializer in investigation.rs)
- Gate corrections use the _correction_issued bool pattern — fire exactly once per turn
- evidence_ready() must remain the single source of truth for evidence state

## Test Rules
- Integration tests: src/runtime/tests/
- Unit tests: inline #[cfg(test)] mod in the file being tested
- One test per behavioral change minimum
- Test must be the regression catch — if it wouldn't have caught the bug, it's not the right test

## Commit Rules
- Never make commits — user always commits manually
- One behavioral change + one test per commit (user enforces this)
- Commit message format: feat/fix(scope): description (Phase X.Y)
