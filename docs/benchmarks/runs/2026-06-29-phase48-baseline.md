# Benchmark Run — 2026-06-29 — Phase 48 Baseline

Date: 2026-06-29
Version: 0.33.83
Backend: openai
Model: gpt-4o-mini
Machine: MacBook Air M2, 8GB RAM

---

## Context

First full regression run since the Phase 39 baseline (2026-06-10, v0.23.72). Six phases landed in between:

- **Phase 40** — constrained decoding (`/constrain`), MLX backend, ability compression (`/compress`)
- **Phases 41–44** — `/refactor`, EditSequence + sequence executor, filesystem watcher, incremental index rebuild on source change, background save verification (RebuildFile)
- **Phase 45** — MCP integration: dynamic tool registration, `MCPManager` (process lifecycle, stdio transport, config loading), dynamic tool calling + surface threading + telemetry
- **Phase 46** — personal memory: storage/schema, `MemoryManager` (embedding recall + keyword fallback), `/remember`, `/forget`, `/memory`, `/reflect`, imperative + reflection filters, MCP polish
- **Phase 47** — tiered shell: `ShellTier` classifier (ReadOnly/FsMutation/Exec), `shell_read` tool, `/exec` toggle, exec-gate intercept, tier-based NL seeding
- **Phase 48** — proactive intelligence: `/dnd` opt-out toggle, proactive config wiring, `stale_facts` query through the memory manager (current branch `feat/proactive-intelligence`)

The test environment now has **two MCP servers configured and alive**: `filesystem` (npx, 14 tools discovered) and `test` (node, 1 tool — `get_weather`). This materially changes the available tool surface versus Phase 39 and is the proximate cause of the two file-creation failures and the answer-leak below.

Tests 1–30 carry over the regression scenarios from the Phase 34 / Phase 39 baselines. Tests 31–39 are new Phase 45–48 validation scenarios (memory, tiered shell, MCP, `/dnd`). The Phase 39 run was 32 scenarios; this run is 40 (two distinct scenarios share the label "Test 23").

---

## Key Behaviors Being Measured

- Investigation path (definition, usage, call site, initialization, config lookups) intact after Phase 40–48 changes
- Mutation path (write, edit, approve, reject) intact — **and** correctly preferred over overlapping MCP filesystem tools
- Anchor resolution and multi-turn context (`that file`, `it`) across turns
- Git read-only surface selection for both slash commands and natural-language phrasings
- LSP definition seeding + `/lsp status` reporting
- **MCP (Phase 45)** — `/mcp list`, dynamic tool registration, approval-gated dynamic tool calls, telemetry
- **Personal memory (Phase 46)** — `/remember` + cross-session recall, `/memory` listing, `/reflect` proposal flow, natural-language imperative memory intent
- **Tiered shell (Phase 47)** — `shell_read` for ReadOnly commands (immediate), `shell` for FsMutation (approval, reversible), Exec default-deny + `/exec on` gate
- **Proactive intelligence (Phase 48)** — `/dnd on` / `/dnd status` opt-out toggle

---

## Results

| Version | Date | Backend | Scenario | Prompt / action | Expected behavior | Observed behavior | Tool rounds | Answer mode | Pass | Notes |
|---------|------|---------|----------|-----------------|-------------------|-------------------|-------------|-------------|------|-------|
| 0.33.83 | 2026-06-29 | openai | InitializationLookup, scoped, truncated | Find where logging is initialized in sandbox/ | Reads init sites, correct answer | Read z_init_target.py + logging_init.py, correct | 3 | ToolAssisted | PASS | useful_target=2; post-evidence tool call correctly rejected. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | DefinitionLookup, scoped, index miss | Find where TaskStatus is defined in sandbox/ | Reads enums.py, correct | index_miss, read enums.py, correct | 2 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | UsageLookup, scoped, truncated | Find where TaskStatus is used in sandbox/ | Usage candidates + definition bypass | Read commands.py, task.py, enums.py (bypass), correct | 4 | ToolAssisted | PASS | answer_guard_rejected (parser.py) once, retry succeeded. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | CallSiteLookup, scoped | Find where load_config is called in sandbox/ | Reads call site, correct | Read main.py, correct | 2 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | CallSiteLookup, scoped | Find where init_logging is called in sandbox/ | Reads call site, correct | Read main.py, correct | 2 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | UsageLookup, scoped | Find where TaskRepository is used in sandbox/ | Usage candidates + definition bypass | Read test_repository.py, main.py, repository.py (bypass), correct | 4 | ToolAssisted | PASS | answer_guard_rejected (task_service.py) once, retry succeeded. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | General, scoped, semantic query | Find where completed tasks are filtered in sandbox/ | Reads relevant files, correct | Read task_service.py + report_service.py (recovery), correct | 3 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | General, direct file query | Find what task_service.py does in sandbox/ | Reads file, describes | Direct read (required=false), correct description | 1 | ToolAssisted | PASS | reason=direct_read. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | General, direct read | Read sandbox/main.py | Reads file, no search | Direct read, no search | 1 | ToolAssisted | PASS | reason=direct_read. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Mutation, create file | Create sandbox/baseline_test.txt | Native write_file, approval, file created | Model called `mcp::filesystem::create_directory` (twice), mcp tool error (254 bytes), tool failed, "Canceled. No action was taken." No file created. | — | — | **FAIL** | **REGRESSION vs Phase 39.** Native write_file path hijacked by MCP filesystem server tools; errors out. No mutation. (Logs block empty in capture.) |
| 0.33.83 | 2026-06-29 | openai | Mutation, edit + approve/reject | Edit sandbox/baseline_test.txt change hello world to hello thunk | edit_file, approval, applied | edit_file approved, replaced 1 line; trailing cargo test (shell) rejected | 1 | ToolAssisted | PASS | MutationEnabled, native edit_file used correctly. cargo test rejected as expected. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Anchor resolution, multi-turn | Read sandbox/config.py → Read that again → Open that again | Re-reads same file on anchor match | anchor_resolved last_read_file on both follow-ups | 1 each | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Git read-only, multi-turn | git status → git diff → git | All three resolve git read-only | status PASS (GitReadOnly), diff PASS (GitReadOnly); bare "git" → surface=AnswerOnly, model tried git_log then git_branch (both disallowed), RepeatedDisallowedTool terminal | 1 / 1 / 2 | ToolAssisted / ToolAssisted / RuntimeTerminal | PARTIAL | status + diff correct. Bare "git" (no object) does not select GitReadOnly; runtime correctly blocked disallowed tools but user gets a failure message. |
| 0.33.83 | 2026-06-29 | openai | DefinitionLookup, scoped, index miss | Find where JsonFileStore is defined in sandbox/ and what it does | Reads definition file, correct | index_miss, read file_store.py, correct + describes methods | 2 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | UsageLookup, low match count | Find where ArgumentParser is used in sandbox/ | Reads usage file, correct | Read parser.py; non-candidate read (models/enums.py) rejected; correct | 3 | ToolAssisted | PASS | non_candidate_read_rejected + post_evidence rejection fired, recovery corrected. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | DefinitionLookup, file-scoped | Find where TaskStatus is defined in sandbox/models/enums.py | Reads scoped file, correct | scope injected as file path, read enums.py, correct | 2 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | DefinitionLookup, no scope, LSP | Where is InvestigationGraph defined? | LSP seeds graph.rs, correct | index_miss, LSP seeded graph.rs line 21, read, correct | 3 | ToolAssisted | PASS | LSP startup delay ~7.6s. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | LSP status, warm session | /lsp status (after Test 17) | Shows running + probe | LSP running, rust-analyzer active, session alive, probe report | — | SystemMessage | PASS | rust-analyzer 1.92.0. |
| 0.33.83 | 2026-06-29 | openai | LSP status, fresh session | /lsp status (fresh session) | Shows enabled, no active session | LSP enabled — no active session, probe report | — | SystemMessage | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | UsageLookup + DefinitionLookup combined | Find where TaskRepository is defined and where it is used in sandbox/ | Usage candidates + definition, correct | Read test_repository.py, main.py, repository.py (bypass); answer_guard_rejected (task_service.py) once, retry succeeded same turn | 4 | ToolAssisted | PASS | **IMPROVEMENT vs Phase 39** (Test 34 was PARTIAL needing a second session; now self-recovers in-turn via post-evidence retry). |
| 0.33.83 | 2026-06-29 | openai | DefinitionLookup, file-scoped, def outside scope | Find where JsonFileStore is defined in sandbox/main.py | Phase 39 terminated via scope guard | scope=sandbox/main.py, read out-of-scope file_store.py (accepted as search_candidate), answered correctly | 2 | ToolAssisted | PASS | **DIVERGENCE vs Phase 39** (Test 35 terminated with answer_scope_guard_rejected). Now answers usefully; scope guard did not reject the out-of-scope definition read. Confirm intended. |
| 0.33.83 | 2026-06-29 | openai | DefinitionLookup, large-file def | Where is run_tool_round defined? | Index hit on 2nd query answers | Q1: read tool_round.rs + investigation.rs, both rejected definition_lookup_non_definition_site → InsufficientEvidence. Q2 same session: index_hit + LSP line 207 → correct | 3 (Q2) | RuntimeTerminal (Q1) / ToolAssisted (Q2) | PARTIAL | Known limitation, unchanged from Phase 39 (Test 14). First query index miss; LSP startup ~7s. |
| 0.33.83 | 2026-06-29 | openai | Mutation, create file (#2) | Create a new file baseline.txt in sandbox/ | Native write_file, approval, file created | Model called `mcp::filesystem::write_file`, approval "Call MCP tool write_file on server filesystem", approved → mcp tool error (416 bytes); answer echoed raw wire block, no file created | 1 | Direct | **FAIL** | **REGRESSION vs Phase 39.** Same MCP-filesystem hijack as Test 10; here via mcp write_file. Native mutation path bypassed; errors. |
| 0.33.83 | 2026-06-29 | openai | Git read-only, NL + cmd + slash | What git branch am I on? → git branch → /git branch | All resolve current branch | NL "what git branch am I on?" → surface=AnswerOnly, git_branch disallowed, fallback text "I cannot access git tools..."; "git branch" → GitReadOnly, correct; "/git branch" → correct | 1 / 1 / — | ToolAssisted / ToolAssisted / SystemMessage | PARTIAL | Bare command + slash work. Natural-language phrasing does not select GitReadOnly surface (same gap as bare "git" in Test 13). |
| 0.33.83 | 2026-06-29 | openai | List dir, NL + slash | List the files in src/runtime/ → /ls src/runtime/ | Lists directory both ways | NL: list_dir executed (17 entries) but assistant answer **leaked the internal context block** ("[thunk: current context] Surface: AnswerOnly Tools: mcp::filesystem::...") instead of a real answer. /ls: correct (9 dirs, 8 files) | 1 / — | ToolAssisted / SystemMessage | PARTIAL | Tool executed + slash command correct, but NL answer is a context-prompt leak. Tool list is flooded with 14 mcp::filesystem::* entries — likely related to the MCP server registration. |
| 0.33.83 | 2026-06-29 | openai | Mutation, edit with search/read + approve | Edit sandbox/main.py adding a comment line → approve | Reads file, edits, approval, applies | search → read main.py → edit_file approved, comment added; cargo test rejected | 3 | ToolAssisted | PASS | Native edit path. Approval widget rendered. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Mutation, edit + approve | Add a comment to the top of sandbox/config.py saying # thunk verified → approve | Edits, approval, applies | search → correction → read config.py → malformed_block_correction → edit_file approved; cargo test rejected | 1 | ToolAssisted | PASS | Self-corrected malformed block once. Native edit path. |
| 0.33.83 | 2026-06-29 | openai | Mutation, verify off + edit | /verify off → Add a comment to sandbox/database.py → approve | No verify output | /verify off set; search → read database.py → edit_file approved; cargo test rejected; no verify fired | 1 | ToolAssisted | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Mutation, verify on + edit | /verify python3 -m py_compile sandbox/main.py → Add a comment to sandbox/utils/time_utils.py → approve | Edit executes, verify fires ok | verify set; edit_file approved; "python3 -m py_compile sandbox/main.py: ok" fires; cargo test rejected | 1 | ToolAssisted | PASS | Verify fires correctly after mutation. Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Mutation, multi-file (grouped) edit | Add # file one to sandbox/config.py and # file two to sandbox/database.py → approve | Two edits / grouped transaction, applied | search config.py → read config.py (not_search_candidate) → search_budget_closed_correction → RepeatedSearchBudgetViolation terminal. No edits produced. | 2 | RuntimeTerminal | **FAIL** | Dual-file edit request never produced any edit_file/transaction; terminated on repeated search budget violation. (Not directly tested in Phase 39 baseline.) |
| 0.33.83 | 2026-06-29 | openai | Slash, transaction + verify status | /transaction → /verify status | "no pending transaction" + verify status | "no pending transaction" / "verify: disabled" | — | SystemMessage | PASS | Matches Phase 39. |
| 0.33.83 | 2026-06-29 | openai | Memory (Phase 46), remember + cross-session recall | /remember My favorite programming language is C → /memory → NEW SESSION → What's my favorite programming language? | Stores fact, lists it, recalls in new session | memory proposal → approve → "Remembered: ... C"; /memory listed 4 facts; new session: "Your favorite programming language is C." | 1 (recall) | Direct | PASS | Recall via AnswerOnly Direct. Proposal/approve flow + cross-session persistence working. |
| 0.33.83 | 2026-06-29 | openai | Memory (Phase 46), /reflect | Conversation about learning Rust → /reflect | Generates fact proposals, approve/reject | /reflect produced sequential proposals; approve/reject each (remembered "wants to learn Rust", "finds Rust intimidating"; discarded generic facts) | — | (memory proposals) | PASS | Reflection proposal pipeline + approve/reject loop working. |
| 0.33.83 | 2026-06-29 | openai | Tiered shell (Phase 47), ReadOnly + FsMutation | run the shell command: ls src/ → run the shell command: mkdir test_dir | ls via shell_read (immediate); mkdir via shell (approval) | ls → shell_read exit 0, immediate, correct listing; mkdir → shell approval-gated, approved, exit 0 | 1 / 1 | ToolAssisted | PASS | Tier classification correct: ReadOnly→shell_read, FsMutation→approval-gated shell. |
| 0.33.83 | 2026-06-29 | openai | Tiered shell (Phase 47), Exec gate | run the shell command: rm test_dir → /exec on → run the shell command: rm test_dir | First denied (exec off); after /exec on, rm runs with approval | First rm → exec-gate intercept "exec mode is disabled", RuntimeTerminal ExecDisabled (correct). After /exec on: second rm **misrouted to search_code** (search_before_answering), no matches, duplicate-search → InsufficientEvidence terminal. rm never executed. | 1 / 2 | RuntimeTerminal / RuntimeTerminal | PARTIAL | Exec default-deny gate works exactly as designed. But post-`/exec on` the model failed to re-issue the shell command (searched instead) — no execution. |
| 0.33.83 | 2026-06-29 | openai | MCP (Phase 45), list + dynamic call | /mcp list → what's the weather in Toronto | Lists servers; calls MCP tool with approval | /mcp list: filesystem (14 tools, alive) + test (1 tool, alive); weather → mcp::test::get_weather approval → approved → "Sunny, 22°C in unknown." | 1 | Direct | PASS | Dynamic tool registration, approval gating, and result threading working. ("unknown" = city arg not populated — minor.) |
| 0.33.83 | 2026-06-29 | openai | Proactive (Phase 48), /dnd | /dnd on → /dnd status | Toggle on, status reflects enabled | "do not disturb enabled — proactive suggestions paused" / "do not disturb: enabled" | — | SystemMessage | PASS | New Phase 48 opt-out toggle working. |
| 0.33.83 | 2026-06-29 | openai | Memory (Phase 46), NL imperative intent | dont forget that I always run cargo fmt before committing | Captured as memory proposal | surface=MutationEnabled; routed to `shell` running "cargo fmt before committing" → shell exit 2 (error). No memory captured. | 1 | ToolAssisted | **FAIL** | `user_requested_remember()` (prompt_analysis.rs:226) matches only `"don't forget "` (with apostrophe) — input "dont forget" misses; `user_requested_execution()` then matches on `run`/`cargo` tokens and routes to shell. Imperative memory intent not captured. |
| 0.33.83 | 2026-06-29 | openai | Multi-turn context + anchor edit | Read sandbox/config.py → what does that file do? → edit it to add a comment at the top | Read, describe from context, edit anchored file | read config.py; "what does that file do?" → Direct, correct description; "edit it" → edit_file on config.py (malformed_block_correction once), approved; cargo test rejected | 1 / 1 / 1 | ToolAssisted / Direct / ToolAssisted | PASS | "that file" / "it" resolved to config.py across turns. Native edit path. |
| 0.33.83 | 2026-06-29 | openai | Tiered shell (Phase 47), Exec + pipe | /exec on → run the shell command: bash -c "ls src/ \| grep rs" | Tier-3 piped cmd approval-gated, runs | exec on; shell approval "run: bash -c ..." (correctly approval-gated as Exec/reversible:false), approved → shell exit 2 | 1 | ToolAssisted | PARTIAL | Gating correct (Tier 3 with pipe → approval, not auto-deny). But command returned exit 2 — execution did not complete cleanly (same exit 2 seen in Test 37). Verify shell quoting/pipe handling. |

---

## Summary

| Result | Count |
|--------|-------|
| PASS    | 30 |
| PARTIAL | 6 |
| FAIL    | 4 |
| **Total** | **40** |

---

## Known Issues

**Tests 10 & 23-create (FAIL) — MCP filesystem tools hijack the native mutation path (REGRESSION)**
With the `filesystem` MCP server alive (14 tools), file-creation prompts route to `mcp::filesystem::create_directory` / `mcp::filesystem::write_file` instead of the native `write_file` tool (`src/tools/core/write_file.rs`). Both error out (254 / 416 byte MCP errors) and no file is created. Phase 39 (no MCP servers) created files correctly via native write_file. This is the highest-impact regression in the run.

**Test 24 (PARTIAL) — natural-language answer leaks internal context block**
"List the files in src/runtime/" executes list_dir correctly but the assistant answer is the raw `[thunk: current context] Surface: ... Tools: mcp::filesystem::...` hint rather than a real answer. The per-turn tool list is flooded with 14 `mcp::filesystem::*` entries; likely the same MCP-registration pressure behind Tests 10/23. The `/ls` slash command is unaffected.

**Test 29 (FAIL) — multi-file grouped edit never materializes**
"Add a comment to config.py AND database.py" terminates on RepeatedSearchBudgetViolation after searching/reading config.py; no edit_file or transaction is ever produced. The grouped/transaction mutation path is not exercised successfully by this prompt.

**Test 37 (FAIL) — natural-language imperative memory intent not captured**
"dont forget that I always run cargo fmt before committing" is not recognized by `user_requested_remember()` (only `"don't forget "` with an apostrophe is matched; there is no "dont forget that" variant). `user_requested_execution()` then matches the `run`/`cargo` tokens and routes to the shell tool (exit 2). The Phase 46 imperative memory filter misses this phrasing.

**Tests 13 & 23-git (PARTIAL) — natural-language / bare git phrasings do not select GitReadOnly**
Bare "git" and "what git branch am I on?" select surface=AnswerOnly, where git tools are disallowed, producing a terminal/fallback. Explicit "git status", "git diff", "git branch", and the `/git ...` slash commands all work. Git surface detection is sensitive to phrasing.

**Test 34 (PARTIAL) — post-`/exec on` command not re-issued**
The Exec default-deny gate fires correctly (first `rm` denied with ExecDisabled). After `/exec on`, the model searches code instead of re-issuing the shell command, ending in InsufficientEvidence; `rm` is never executed. Gate behavior is correct; the follow-through is a model-behavior failure.

**Test 39 (PARTIAL) — Tier-3 piped command exits 2**
`bash -c "ls src/ | grep rs"` is correctly classified Exec, approval-gated (not auto-denied with `/exec on`), and approved — but returns exit 2. Same nonzero exit observed in Test 37. Worth checking how the shell tool parses quoted/piped command strings.

**Test 22 (PARTIAL) — large-file definition lookup, first-query miss (unchanged)**
Carried over from Phase 39 (Test 14). First query index-misses and exhausts candidate reads (definition_lookup_non_definition_site on two large files); second query in the same session gets an index_hit + LSP and answers correctly.

**Test 21 (PASS, behavioral divergence) — out-of-scope definition now answered**
File-scoped lookup whose definition lives outside the scope file now reads the out-of-scope file and answers (Phase 39 Test 35 terminated via answer_scope_guard). Useful outcome, but the scope guard did not reject — confirm this is intended and not a weakened gate.

---

## Triage List (for separate investigation)

| Test | Verdict | What went wrong | One-line issue |
|------|---------|-----------------|----------------|
| 10 | FAIL | `Create sandbox/baseline_test.txt` routed to `mcp::filesystem::create_directory`, errored (254B), no file created | MCP filesystem tools hijack native write_file mutation path (regression). |
| 23-create | FAIL | `Create a new file baseline.txt in sandbox/` routed to `mcp::filesystem::write_file`, approved → mcp error (416B), no file | Same MCP-filesystem hijack via mcp write_file; answer echoed raw wire block. |
| 29 | FAIL | Dual-file edit (config.py + database.py) → RepeatedSearchBudgetViolation terminal, zero edits | Multi-file/grouped edit prompt never reaches edit_file or transaction path. |
| 37 | FAIL | "dont forget that I always run cargo fmt..." → routed to shell (exit 2), no memory stored | Imperative memory filter misses "dont forget" (apostrophe-less); execution detector wins on run/cargo. |
| 13 | PARTIAL | Bare "git" → AnswerOnly surface, git_log/git_branch disallowed, RepeatedDisallowedTool terminal | Bare "git" doesn't select GitReadOnly; status/diff fine. |
| 23-git | PARTIAL | "What git branch am I on?" → AnswerOnly, git_branch disallowed, fallback text | NL git phrasing doesn't select GitReadOnly; "git branch" + "/git branch" fine. |
| 24 | PARTIAL | NL "List the files in src/runtime/" answer leaked internal `[thunk: current context]` block | list_dir ran + /ls fine, but NL answer is a context-prompt leak; tool list flooded with mcp::filesystem::*. |
| 34 | PARTIAL | After `/exec on`, second `rm test_dir` searched code instead of running → InsufficientEvidence | Exec deny gate correct; model fails to re-issue shell command post-enable. |
| 39 | PARTIAL | `bash -c "ls src/ \| grep rs"` approved but exit 2 | Tier-3 gating correct; quoted/piped command execution returns nonzero. |
| 22 | PARTIAL | `Where is run_tool_round defined?` first query InsufficientEvidence; 2nd succeeds | Known limitation (unchanged from Phase 39 Test 14); first-query index miss on large file. |
