//! Integration tests for Slice 47.6 — shell execution feedback and post-seed
//! answer admission.

use std::fs;
use tempfile::TempDir;

use crate::runtime::types::RuntimeTerminalReason;

use super::*;

fn system_messages(events: &[RuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::SystemMessage(msg) = e {
                Some(msg.clone())
            } else {
                None
            }
        })
        .collect()
}

fn tool_call_started_names(events: &[RuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::ToolCallStarted { name } = e {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Bug #1: a Tier-3 shell command denied because exec mode is disabled must surface
/// the denial to the user as a SystemMessage AND end the turn immediately (no
/// deny→retry spiral, which a `continue` in the exec gate previously caused).
#[test]
fn exec_gate_surfaces_message_to_user() {
    let tmp = TempDir::new().unwrap();
    // No backend responses needed — the seeded Tier-3 shell call is denied by the
    // exec gate before any synthesis pass. exec_enabled defaults to false.
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());

    // Note: the command token must not look like a code identifier (e.g. snake_case),
    // or prompt_requires_investigation suppresses the seed. "temp" is a safe Tier-3 target.
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "run the shell command: rm temp".to_string(),
        },
    );

    // The denial reason must reach the user as a SystemMessage, not just the
    // model-facing accumulated buffer.
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("exec mode is disabled")),
        "expected exec-disabled SystemMessage to surface to user; got: {msgs:?}"
    );

    // The turn must terminate immediately with the ExecDisabled terminal reason.
    let terminated = events.iter().any(|e| {
        matches!(
            e,
            RuntimeEvent::AnswerReady(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::ExecDisabled,
                ..
            })
        )
    });
    assert!(
        terminated,
        "expected RuntimeTerminal(ExecDisabled) answer ending the turn; got: {events:?}"
    );
}

/// Bug #2: after a seeded read-only shell command (shell_read) completes, the runtime
/// must enter the PostRead answer phase so the model synthesizes from the result
/// instead of continuing to investigate. A subsequent tool call must be blocked.
#[test]
fn post_seed_shell_read_sets_answer_phase() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/foo.rs"), "fn main() {}\n").unwrap();

    // Round 0: seeded shell_read executes (no backend call).
    // Backend call 1: model attempts another tool — must be rejected by the PostRead gate.
    // Backend call 2: model produces the synthesis answer — admitted.
    let mut rt = make_runtime_in(
        vec!["[search_code: needle]", "Here are the files in src."],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "run the shell command: ls src/".to_string(),
        },
    );

    let started = tool_call_started_names(&events);
    // The seed must have executed the read-only shell tool.
    assert!(
        started.iter().any(|n| n == "shell_read"),
        "expected shell_read to be seeded and executed; started tools: {started:?}"
    );
    // The post-seed tool call must be blocked by the PostRead answer phase: without the
    // fix, answer_phase stays None and search_code would dispatch.
    assert!(
        !started.iter().any(|n| n == "search_code"),
        "post-seed tool call must be blocked by PostRead answer phase; started tools: {started:?}"
    );

    // The synthesis answer must be admitted.
    let answer = assistant_chunks(&events).join("");
    assert!(
        answer.contains("Here are the files in src."),
        "expected synthesized answer to be admitted; got chunks: {:?}",
        assistant_chunks(&events)
    );
}

/// Bug fix E: shell seed must fire even when the command argument is a snake_case
/// identifier (e.g. "rm test_dir"). Previously, prompt_requires_investigation saw
/// "test_dir" as a code identifier and set investigation_required=true, which
/// suppressed the seed gate.
#[test]
fn shell_seed_fires_with_snake_case_argument() {
    let tmp = TempDir::new().unwrap();
    // exec_enabled defaults to false — the Tier-3 (rm) command will be denied by the exec gate,
    // but only AFTER the seed path fires and reaches the gate. This confirms the seed was placed.
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "run rm test_dir".to_string(),
        },
    );

    // The exec-gate denial SystemMessage proves the shell seed fired (exec gate only triggers
    // after the seeded shell command reaches it).
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("exec mode is disabled")),
        "expected exec-disabled message proving shell seed fired for snake_case arg; got: {msgs:?}"
    );
}
