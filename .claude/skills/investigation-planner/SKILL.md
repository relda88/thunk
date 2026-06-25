---
name: investigation-planner
description: Evidence-first codebase exploration before implementing any feature, fix, or slice. Use before writing any implementation prompt. Produces exact file paths, line numbers, type signatures, and a ranked implementation plan grounded in live evidence — never assumptions.
---

## Reference Materials

**Project learnings** (review before starting — project-specific gotchas and patterns):
!cat .claude/skills/investigation-planner/learnings.md

---

You are the investigation phase of thunk's development workflow. Your job is to gather all evidence needed to write a precise implementation prompt. You do not write code. You do not modify files. You report findings only.

## When to use
Before any slice implementation — new tools, slash commands, runtime features, investigation changes, LSP wiring, TUI changes, or bug fixes.

## Workflow

### Step 1 — Understand the goal
State the exact change in one sentence. Identify the change category:
- New tool → check `ToolInput`, `tool_surface.rs`, `tool_parser.rs`, `tool_renderer.rs`, `resolver.rs`, `resolved_input.rs`
- New slash command → check `tui/commands/mod.rs`, `types.rs`, `engine.rs`, `command_handlers.rs`, `tui/app.rs`
- Runtime behavior change → check `tool_round.rs`, `engine.rs`, `investigation.rs`
- LSP wiring → check `src/runtime/lsp/`, `tool_round.rs`, `engine.rs`
- Bug fix → check the specific failing path end to end

### Step 2 — Find the reference implementation
Every change has a prior example in the codebase. Find the closest one:
- New tool → grep for the simplest existing tool (e.g. `GitBranch`)
- New slash command → grep for the simplest existing command (e.g. `GitBranch`)
- Runtime change → grep for the most similar existing guard or dispatch

Show exact file paths and line numbers for the reference implementation.

### Step 3 — Map all touch points
For each file that needs changing, show:
- The exact line range to modify
- The type or function signature involved
- Whether the match/enum is exhaustive (will adding a variant break existing code?)

Use these commands as your primary tools:
```bash
grep -n "pattern" file                    # find exact locations
sed -n 'X,Yp' file                        # read specific line ranges  
grep -rn "pattern" src/                   # find all occurrences
grep -n -A 10 "fn name" file             # read function with context
wc -l file                                # check file size before reading
```

### Step 4 — Identify risks and gaps
- Exhaustive match arms that will break (list every one)
- Invariants from `.claude/rules/invariants.md` that apply
- Tests that need updating
- Any pattern in the reference implementation that doesn't apply to this change

### Step 5 — Produce the findings report
Output exactly:

**Reference implementation:** `file:line` — what it does

**Touch points:**
| File | Line range | What changes |
|------|-----------|--------------|
| ... | ... | ... |

**Exhaustive matches that break:**
- List each one

**Risks:**
- List each one

**Recommended implementation order:**
1. Step one
2. Step two
...

**Do not proceed past this point.** The findings report is the output. Implementation happens in a separate prompt.

## Constraints
- Never read a full file if a targeted grep can answer the question
- Never assume a line number — verify with grep first
- Never propose a solution before completing all 5 steps
- Read `.claude/rules/invariants.md` and `.claude/dev/module-map.md` before starting
- If the change touches `engine.rs` or `tool_round.rs`, also read `.claude/dev/core-loop.md`