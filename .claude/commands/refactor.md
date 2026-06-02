# /refactor

Analyze the codebase for files and functions that should be split for
modularity, separation of concerns, and maintainability.

## Usage
- `/refactor` — scan all source files, report anything over threshold
- `/refactor src/runtime/orchestration/tool_round.rs` — analyze specific file
- `/refactor 300` — use custom line threshold instead of default 500

## Steps

1. Read `.claude/rules/invariants.md` and `.claude/dev/module-map.md` first
2. If a specific file was given, analyze that file only
3. Otherwise, find all `.rs` files over the line threshold:
   `find src -name "*.rs" | xargs wc -l | sort -rn | head -20`
4. For each file over threshold:
   - List distinct responsibilities it owns
   - Identify functions over 100 lines
   - Flag any separation of concerns violations
   - Flag any layering violations per module-map.md
5. For each candidate split:
   - Propose new module name and what moves there
   - Estimate risk: low / medium / high
   - Note any cross-module import changes required
6. Output a prioritized list — highest risk files first

## Constraints
- Never suggest splitting for line count alone — only when distinct
  responsibilities exist
- Never propose changes that violate `.claude/rules/invariants.md`
- Flag any split that touches public APIs or cross-module imports
- Do not modify any files — analysis only unless explicitly asked