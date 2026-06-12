use super::*;
use crate::core::config::Config;
use crate::llm::backend::GenerateRequest;
use crate::runtime::types::RuntimeTerminalReason;
use crate::tools::default_registry;
use std::sync::{Arc, Mutex};

fn make_runtime_in_with_recorded_requests(
    responses: Vec<impl Into<String>>,
    root: &std::path::Path,
) -> (Runtime, Arc<Mutex<Vec<GenerateRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let project_root = ProjectRoot::new(root.to_path_buf()).unwrap();
    let runtime = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(RecordingBackend::new(responses, Arc::clone(&requests))),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-session".to_string(),
    );
    (runtime, requests)
}

#[test]
fn approve_with_no_pending_fires_failed() {
    let mut rt = make_runtime(vec!["hello"]);
    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(has_failed(&events), "expected Failed, got: {events:?}");
    assert_eq!(
        failed_message(&events).as_deref(),
        Some("No pending action to approve.")
    );
}

#[test]
fn reject_with_no_pending_fires_failed() {
    let mut rt = make_runtime(vec!["hello"]);
    let events = collect_events(&mut rt, RuntimeRequest::Reject);
    assert!(has_failed(&events), "expected Failed, got: {events:?}");
    assert_eq!(
        failed_message(&events).as_deref(),
        Some("No pending action to reject.")
    );
}

#[test]
fn reject_uses_runtime_cancellation_even_if_model_would_claim_success() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(
        vec![
            "[write_file]\npath: reject_test_phase75.txt\n---content---\nshould not exist\n[/write_file]",
            "I created reject_test_phase75.txt.",
        ],
        tmp.path(),
    );

    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Create a file reject_test_phase75.txt with the content should not exist".into(),
        },
    );
    assert!(
        !has_failed(&submit_events),
        "submit failed: {submit_events:?}"
    );
    assert!(
        submit_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. })),
        "write_file must request approval"
    );

    let reject_events = collect_events(&mut rt, RuntimeRequest::Reject);
    assert!(
        !has_failed(&reject_events),
        "reject failed: {reject_events:?}"
    );
    assert!(
        !tmp.path().join("reject_test_phase75.txt").exists(),
        "rejected write must not create the file"
    );

    let snapshot = rt.messages_snapshot();
    // Runtime owns the cancellation answer — the model's synthesis response must not be used.
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("Canceled. No file was created")),
        "runtime cancellation answer must be recorded"
    );
    assert!(
        !snapshot
            .iter()
            .any(|m| m.content.contains("I created reject_test_phase75.txt.")),
        "backend response after reject must not be used"
    );
    assert!(fs::read_dir(tmp.path()).unwrap().next().is_none());
}

#[test]
fn submit_while_pending_fires_failed() {
    let mut rt = make_runtime(vec!["hello"]);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: "edit src/lib.rs".into(),
        risk: RiskLevel::Medium,
        payload: "{}".into(),
    });
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "continue".into(),
        },
    );
    assert!(has_failed(&events), "expected Failed, got: {events:?}");
    assert!(failed_message(&events)
        .as_deref()
        .unwrap_or("")
        .contains("pending"),);
}

#[test]
fn reset_clears_pending_state() {
    let mut rt = make_runtime(vec!["hello"]);
    rt.set_pending_for_test(PendingAction {
        tool_name: "write_file".into(),
        summary: "write src/new.rs".into(),
        risk: RiskLevel::High,
        payload: "{}".into(),
    });
    collect_events(&mut rt, RuntimeRequest::Reset);
    // After reset, approve should fail with "no pending" — not "submit blocked"
    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        has_failed(&events),
        "expected Failed after reset, got: {events:?}"
    );
    assert_eq!(
        failed_message(&events).as_deref(),
        Some("No pending action to approve.")
    );
}

#[test]
fn edit_repair_correction_injected_on_garbled_repair_after_failure() {
    // First response: edit_file with empty search text — produces an Immediate tool error.
    // Second response: [edit_file] tags present but unrecognized delimiters (zero parse).
    // Engine must inject EDIT_REPAIR_CORRECTION rather than accepting as Direct.
    // Third response: synthesis after correction.
    let bad_edit = "[edit_file]\npath: foo.rs\n---replace---\nnew text\n[/edit_file]";
    let garbled_repair =
        "[edit_file]\npath: foo.rs\nFind: old text\nReplace: new text\n[/edit_file]";
    let synthesis = "I was unable to apply the edit.";

    let mut rt = make_runtime(vec![bad_edit, garbled_repair, synthesis]);
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "edit foo.rs".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "must not fail permanently: {events:?}"
    );

    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.starts_with("[runtime:correction]")
                && m.content.contains("edit_file")),
        "edit repair correction must be injected: {snapshot:?}"
    );
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(last_assistant, Some(synthesis));
}

#[test]
fn repeated_garbled_edit_repair_terminals_without_surfacing_malformed_block() {
    let bad_edit = "[edit_file]\npath: foo.rs\n---replace---\nnew text\n[/edit_file]";
    let garbled_repair =
        "[edit_file]\npath: foo.rs\nFind: old text\nReplace: new text\n[/edit_file]";

    let mut rt = make_runtime(vec![
        bad_edit,
        garbled_repair,
        garbled_repair,
        "This response should not be consumed.",
    ]);
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "edit foo.rs".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "repeated garbled edit repair must terminate cleanly: {events:?}"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(
            answer_source,
            Some(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::RepeatedGarbledEditRepair,
                ..
            })
        ),
        "second garbled edit repair must use deterministic runtime terminal: {answer_source:?}"
    );

    let snapshot = rt.messages_snapshot();
    let assistant_messages: Vec<&str> = snapshot
        .iter()
        .filter(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        !assistant_messages
            .iter()
            .any(|m| m.contains("Find: old text") || m.contains("Replace: new text")),
        "garbled edit repair must not surface as a final assistant answer: {assistant_messages:?}"
    );
    let last_assistant = assistant_messages.last().copied();
    assert!(
        matches!(last_assistant, Some(s) if s.contains("invalid edit_file repair block")),
        "last assistant message must be the runtime garbled-repair terminal: {last_assistant:?}"
    );
}

#[test]
fn edit_old_new_content_format_requests_approval_and_executes() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("test_phase82.txt");
    fs::write(&file, "hello world").unwrap();

    let edit = "[edit_file]\npath: test_phase82.txt\nold content: hello world\nnew content: hello thunk\n[/edit_file]";
    let mut rt = make_runtime_in(vec![edit, "Updated."], tmp.path());

    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Edit test_phase82.txt and change hello world to hello thunk".into(),
        },
    );
    assert!(
        !has_failed(&submit_events),
        "submit failed: {submit_events:?}"
    );
    assert!(
        submit_events.iter().any(
            |e| matches!(e, RuntimeEvent::ApprovalRequired { pending: p, .. }
            if p.tool_name == "edit_file")
        ),
        "edit must request approval instead of falling back to Direct: {submit_events:?}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve failed: {approve_events:?}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello thunk");

    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: edit_file ===")),
        "approved edit result must be injected: {snapshot:?}"
    );
}

#[test]
fn simple_edit_prompt_seeds_edit_file_and_requests_approval() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("test.txt");
    fs::write(&file, "hello world").unwrap();

    let (mut rt, requests) =
        make_runtime_in_with_recorded_requests(vec!["should not be used"], tmp.path());
    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Edit the file test.txt replace the content hello world with hello thunk".into(),
        },
    );

    assert!(
        !has_failed(&submit_events),
        "submit failed: {submit_events:?}"
    );
    assert!(
        submit_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { pending: p, .. } if p.tool_name == "edit_file")),
        "simple edit prompt must request edit_file approval: {submit_events:?}"
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "seeded simple edit must reach approval before any model generation"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "hello world",
        "file must not change before approval"
    );
}

#[test]
fn seeded_simple_edit_executes_only_after_approval() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("hello.txt");
    fs::write(&file, "hello root").unwrap();

    let (mut rt, requests) =
        make_runtime_in_with_recorded_requests(vec!["still unused"], tmp.path());
    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Edit hello.txt replace hello root with hello runtime".into(),
        },
    );

    assert!(
        !has_failed(&submit_events),
        "submit failed: {submit_events:?}"
    );
    assert!(
        submit_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { pending: p, .. } if p.tool_name == "edit_file")),
        "seeded simple edit must enter the normal approval path: {submit_events:?}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "hello root",
        "file must not change before approval"
    );

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve failed: {approve_events:?}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "hello runtime",
        "seeded simple edit must execute only after approval"
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "seeded simple edit must stay on the runtime-owned resolver/approval path"
    );
}

#[test]
fn simple_edit_prompt_outside_root_is_rejected_before_approval() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().parent().unwrap().join("outside.txt");

    let (mut rt, requests) =
        make_runtime_in_with_recorded_requests(vec!["must not be used"], tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: format!(
                "Edit {} replace hello world with hello thunk",
                outside.display()
            ),
        },
    );

    assert!(!has_failed(&events), "must terminate cleanly: {events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. })),
        "outside-root seeded simple edit must terminate before approval: {events:?}"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(
            answer_source,
            Some(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::MutationFailed,
                ..
            })
        ),
        "outside-root seeded simple edit must end as MutationFailed: {answer_source:?}"
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "outside-root seeded simple edit must terminate before any model generation"
    );
}

#[test]
fn and_change_form_goes_straight_to_approval() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("baseline_test.txt");
    fs::write(&file, "hello world").unwrap();

    let (mut rt, requests) =
        make_runtime_in_with_recorded_requests(vec!["should not be used"], tmp.path());
    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Edit baseline_test.txt and change hello world to hello thunk".into(),
        },
    );

    assert!(
        !has_failed(&submit_events),
        "submit failed: {submit_events:?}"
    );
    assert!(
        submit_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { pending: p, .. } if p.tool_name == "edit_file")),
        "and-change form must request edit_file approval: {submit_events:?}"
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "and-change form must reach approval before any model generation"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "hello world",
        "file must not change before approval"
    );
}

#[test]
fn approve_produces_runtime_owned_answer_after_successful_mutation() {
    // After approving a mutation, the runtime must finalize directly without
    // re-entering model generation. The answer is built from the tool output summary.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("hello.txt");
    std::fs::write(&path, "hello\n").unwrap();
    let path = path.to_string_lossy().into_owned();
    let payload = format!("{}\x00hello\x00world", path);

    // No model responses needed — the runtime owns the answer.
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());
    let before_count = rt.messages_snapshot().len();

    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {path}"),
        risk: RiskLevel::Medium,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not fail: {events:?}");

    // finish_with_runtime_answer emits AssistantMessageChunk for the runtime-owned answer.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::AssistantMessageChunk(_))),
        "runtime-owned answer must emit AssistantMessageChunk"
    );

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(
            answer_source,
            Some(AnswerSource::ToolAssisted { rounds: 1 })
        ),
        "mutation finalization must use ToolAssisted {{ rounds: 1 }}: {answer_source:?}"
    );

    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot.len() > before_count,
        "snapshot must grow after approve + runtime finalization"
    );
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: edit_file ===")),
        "tool result must be in conversation after approve"
    );
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant);
    assert!(
        last_assistant
            .map(|m| m.content.starts_with("edit_file result:"))
            .unwrap_or(false),
        "last assistant message must be the runtime-owned mutation answer: {last_assistant:?}"
    );
}

#[test]
fn mutation_turn_with_preparatory_read_still_reaches_edit_file_approval() {
    // Regression test for Fix 2: answer_phase must not fire on mutation-allowed turns
    // after a preparatory read, or the model can never proceed to call edit_file.
    //
    // Sequence: model reads target file first (confirming content), then calls edit_file.
    // Both calls must be allowed — the PostRead answer_phase gate must not intercept.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("hello.txt");
    fs::write(&target, "hello root\n").unwrap();

    let read_then_edit = vec![
        "[read_file: hello.txt]",
        "[edit_file]\npath: hello.txt\n---search---\nhello root\n---replace---\nhello runtime\n[/edit_file]",
        "Done.",
    ];
    let mut rt = make_runtime_in(read_then_edit, tmp.path());

    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Edit hello.txt and change hello root to hello runtime".into(),
        },
    );

    assert!(
        !has_failed(&submit_events),
        "mutation turn with prior read must not fail: {submit_events:?}"
    );
    assert!(
        submit_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { pending: p, .. } if p.tool_name == "edit_file")),
        "edit_file must reach approval even after a preparatory read: {submit_events:?}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "hello root\n",
        "file must not be modified before approval"
    );

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve must succeed: {approve_events:?}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "hello runtime\n",
        "file must be updated after approval"
    );
}

#[test]
fn diagnostics_not_injected_when_lsp_disabled() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("lib.rs");
    fs::write(&file, "fn hello() {}\n").unwrap();
    let abs_path = file.to_string_lossy().into_owned();
    let payload = format!("{}\x00fn hello()\x00fn world()", abs_path);

    // Config::default() has lsp.enabled = false — diagnostics must not be injected.
    // Disable corrections (tmpdir has no Cargo.toml; this test is not about corrections).
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path()).with_max_correction_attempts(0);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Medium,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not fail: {events:?}");

    let snapshot = rt.messages_snapshot();
    assert!(
        !snapshot
            .iter()
            .any(|m| m.content.contains("lsp_diagnostics")),
        "lsp_diagnostics must not appear when LSP is disabled: {snapshot:?}"
    );
}

// When LSP is disabled (Config::default()), the pre-edit safety check is skipped.
// Approve fires once → mutation executes immediately; no second ApprovalRequired is emitted.
// This is the regression test for Slice 34.1: the pre-check gate must not affect
// any existing approval path when LSP is off.
//
// When test infrastructure gains mock LSP support, add a companion test that enables
// LSP, injects errors, and verifies the second-approval re-prompt path.
#[test]
fn lsp_disabled_pre_check_skipped_mutation_executes_in_one_approval() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("lib.rs");
    fs::write(&file, "fn foo() {}\n").unwrap();
    let abs_path = file.to_string_lossy().into_owned();
    // Legacy payload format: abs_path\x00search\x00replace
    let payload = format!("{abs_path}\x00fn foo()\x00fn bar()");

    // Config::default() has lsp.enabled = false — pre-check must be bypassed.
    // Disable corrections (tmpdir has no Cargo.toml; this test is not about corrections).
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path()).with_max_correction_attempts(0);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);

    let re_approval_count = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. }))
        .count();
    assert_eq!(
        re_approval_count, 0,
        "pre-check must not re-issue ApprovalRequired when LSP is disabled: {events:?}"
    );
    assert!(
        !has_failed(&events),
        "approve must succeed when LSP is disabled: {events:?}"
    );
}

#[test]
fn verify_emits_system_message_after_mutation() {
    // After an approved edit_file mutation on a .rs file with verify_after_mutation
    // enabled, the runtime must emit at least one SystemMessage containing "cargo check".
    // Uses a real tmpdir project so cargo check has a valid manifest to run against.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"verify-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let main_rs = src.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    let abs_path = main_rs.to_string_lossy().into_owned();
    // Use the full "fn main() {}" as old content so the replacement doesn't leave stray "{}".
    let payload = format!("{abs_path}\x00fn main() {{}}\x00fn main() {{ let _x = 1; }}");

    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path())
        .with_verify_command(Some("cargo check".into()))
        .with_max_correction_attempts(0);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not fail: {events:?}");

    let has_cargo_check_msg = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("cargo check")));
    assert!(
        has_cargo_check_msg,
        "must emit a SystemMessage containing 'cargo check' when verify is enabled: {events:?}"
    );
}

#[test]
fn verify_skipped_when_disabled() {
    // When verify_after_mutation is false, no SystemMessage containing "cargo check"
    // must be emitted, even for a .rs file mutation.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"verify-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let main_rs = src.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    let abs_path = main_rs.to_string_lossy().into_owned();
    let payload = format!("{abs_path}\x00fn main()\x00fn main() {{ let _x = 1; }}");

    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path()).with_verify_command(None);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not fail: {events:?}");

    let has_cargo_check_msg = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("cargo check")));
    assert!(
        !has_cargo_check_msg,
        "must not emit 'cargo check' SystemMessage when verify is disabled: {events:?}"
    );
}

#[test]
fn correction_loop_emits_approval_on_first_failure() {
    // After an approved mutation that fails cargo check, and with corrections enabled,
    // the runtime must inject a correction prompt, get a corrective edit from the model,
    // and emit ApprovalRequired for that corrective edit. Approving the corrective edit
    // must complete the turn with AnswerReady.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"corr-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();
    let abs_path = main_rs.to_string_lossy().into_owned();

    // Initial edit introduces a type error. Payload: abs_path\x00old\x00new.
    let initial_payload =
        format!("{abs_path}\x00fn main() {{}}\x00fn main() {{ let x: i32 = \"bad\"; }}");

    // The corrective edit the mock backend will propose. Use a relative path so the
    // resolver does not hit the /tmp vs /private/tmp symlink mismatch on macOS.
    let corrective_edit =
        "[edit_file]\npath: src/main.rs\nold content: let x: i32 = \"bad\";\nnew content: let _x: i32 = 1;\n[/edit_file]";
    let (rt, _) =
        make_runtime_in_with_recorded_requests(vec![corrective_edit, "Fixed."], tmp.path());
    let mut rt = rt
        .with_verify_command(Some("cargo check".into()))
        .with_max_correction_attempts(2);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload: initial_payload,
    });

    // First Approve: executes original (broken) edit, cargo check fails, correction requested.
    let first_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&first_events),
        "first approve must not fail: {first_events:?}"
    );
    assert!(
        first_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("requesting correction (attempt 1/2)"))),
        "must emit correction request SystemMessage: {first_events:?}"
    );
    assert!(
        first_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. })),
        "must emit ApprovalRequired for the corrective edit: {first_events:?}"
    );

    // Second Approve: executes the corrective edit; cargo check should pass now.
    let second_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&second_events),
        "second approve must not fail: {second_events:?}"
    );
    assert!(
        second_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::AnswerReady(_))),
        "second approve must complete with AnswerReady: {second_events:?}"
    );
}

#[test]
fn correction_exhaustion_emits_summary() {
    // When the model responds with prose instead of an edit after a correction prompt,
    // the runtime must emit an exhaustion SystemMessage containing "manual fix required"
    // and complete the turn with AnswerReady — no infinite loop.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"exhaust-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();
    let abs_path = main_rs.to_string_lossy().into_owned();

    // Initial edit introduces a type error.
    let initial_payload =
        format!("{abs_path}\x00fn main() {{}}\x00fn main() {{ let x: i32 = \"bad\"; }}");

    // Backend responds with prose — no edit_file tool call.
    let (rt, _) = make_runtime_in_with_recorded_requests(
        vec!["Sorry, I cannot fix this automatically."],
        tmp.path(),
    );
    let mut rt = rt
        .with_verify_command(Some("cargo check".into()))
        .with_max_correction_attempts(1);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload: initial_payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not fail: {events:?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            RuntimeEvent::SystemMessage(msg) if msg.contains("manual fix required")
        )),
        "must emit exhaustion SystemMessage: {events:?}"
    );
    // AnswerReady must fire exactly once — no double-fire from run_turns + outer finish.
    let answer_ready_count = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::AnswerReady(_)))
        .count();
    assert_eq!(
        answer_ready_count, 1,
        "AnswerReady must fire exactly once: {events:?}"
    );
}

// ---- Transaction tests (Slice 34.4) ----------------------------------------

#[test]
fn transaction_produces_grouped_approval() {
    use crate::runtime::types::RuntimeEvent;
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file_a.py"), "old_a\n").unwrap();
    fs::write(tmp.path().join("file_b.py"), "old_b\n").unwrap();

    let two_edits = format!(
        "[edit_file]\npath: file_a.py\n---search---\nold_a\n---replace---\nnew_a\n[/edit_file]\n\
         [edit_file]\npath: file_b.py\n---search---\nold_b\n---replace---\nnew_b\n[/edit_file]"
    );

    let mut rt = make_runtime_in(vec![two_edits], tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "edit both files".into(),
        },
    );

    assert!(!has_failed(&events), "submit must not fail: {events:?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            RuntimeEvent::TransactionApprovalRequired { actions, .. }
            if actions.len() == 2
        )),
        "must fire TransactionApprovalRequired with 2 actions: {events:?}"
    );
}

#[test]
fn transaction_executes_atomically() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file_a.py"), "old_a\n").unwrap();
    fs::write(tmp.path().join("file_b.py"), "old_b\n").unwrap();

    let two_edits = format!(
        "[edit_file]\npath: file_a.py\n---search---\nold_a\n---replace---\nnew_a\n[/edit_file]\n\
         [edit_file]\npath: file_b.py\n---search---\nold_b\n---replace---\nnew_b\n[/edit_file]"
    );

    let mut rt = make_runtime_in(vec![two_edits], tmp.path());
    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "edit both files".into(),
        },
    );

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve must not fail: {approve_events:?}"
    );
    assert!(
        approve_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::AnswerReady(_))),
        "AnswerReady must fire after transaction: {approve_events:?}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("file_a.py"))
            .unwrap()
            .trim(),
        "new_a",
        "file_a.py must be updated"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("file_b.py"))
            .unwrap()
            .trim(),
        "new_b",
        "file_b.py must be updated"
    );
}

#[test]
fn transaction_rolls_back_on_failure() {
    // Scenario: model proposes two valid edits. After approval is shown to the user,
    // file_b.py is modified externally (simulating a concurrent write). On Approve,
    // the first edit succeeds, the second fails the staleness check in execute_approved(),
    // and the runtime rolls back the first edit.
    use crate::runtime::types::RuntimeEvent;
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file_a.py"), "old_a\n").unwrap();
    fs::write(tmp.path().join("file_b.py"), "old_b\n").unwrap();

    // Both search texts exist at Submit time so both pass EditFileTool::run().
    let two_edits = format!(
        "[edit_file]\npath: file_a.py\n---search---\nold_a\n---replace---\nnew_a\n[/edit_file]\n\
         [edit_file]\npath: file_b.py\n---search---\nold_b\n---replace---\nnew_b\n[/edit_file]"
    );

    let mut rt = make_runtime_in(vec![two_edits], tmp.path());
    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "edit both files".into(),
        },
    );
    assert!(
        submit_events.iter().any(|e| matches!(
            e,
            RuntimeEvent::TransactionApprovalRequired { actions, .. }
            if actions.len() == 2
        )),
        "must fire TransactionApprovalRequired: {submit_events:?}"
    );

    // Simulate external modification of file_b.py after proposal but before approval.
    // The staleness check in execute_approved() will fail because "old_b" is gone.
    fs::write(tmp.path().join("file_b.py"), "externally_modified\n").unwrap();

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve must not emit Failed even on rollback: {approve_events:?}"
    );
    assert!(
        approve_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::AnswerReady(_))),
        "AnswerReady must fire so the turn completes: {approve_events:?}"
    );
    assert!(
        approve_events.iter().any(|e| {
            if let RuntimeEvent::SystemMessage(msg) = e {
                msg.contains("rolled back")
            } else {
                false
            }
        }),
        "must emit rolled back system message: {approve_events:?}"
    );
    // file_a.py must be restored to its original content after rollback.
    assert_eq!(
        fs::read_to_string(tmp.path().join("file_a.py"))
            .unwrap()
            .trim(),
        "old_a",
        "file_a.py must be rolled back to original content"
    );
}

#[test]
fn edit_file_approval_carries_impact_when_importer_exists() {
    use rusqlite::Connection;
    use tempfile::{NamedTempFile, TempDir};

    use crate::runtime::ProjectRoot;
    use crate::storage::index::types::ImportEdge;
    use crate::storage::index::SymbolStore;
    use crate::storage::session::schema;

    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/target.rs"), "fn foo() {}\n").unwrap();

    // Initialize DB and seed an importer edge before the runtime opens it.
    let db_file = NamedTempFile::new().unwrap();
    {
        let conn = Connection::open(db_file.path()).unwrap();
        schema::initialize(&conn).unwrap();
    }
    let store = SymbolStore::open(db_file.path()).unwrap();
    let canonical_root = tmp.path().canonicalize().unwrap();
    let root_str = canonical_root.to_string_lossy().to_string();
    store
        .upsert_imports(
            &root_str,
            &[ImportEdge {
                from_file: "src/importer.rs".to_string(),
                to_file: "src/target.rs".to_string(),
            }],
        )
        .unwrap();
    drop(store);

    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(TestBackend::new(vec![
            "[edit_file]\npath: src/target.rs\n---search---\nfn foo() {}\n---replace---\nfn bar() {}\n[/edit_file]",
        ])),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        tmp.path().to_path_buf(),
        "test-impact".to_string(),
    )
    .with_symbol_store(db_file.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "replace foo with bar in src/target.rs".into(),
        },
    );

    let impact = events.iter().find_map(|e| {
        if let RuntimeEvent::ApprovalRequired { impact, .. } = e {
            Some(impact.clone())
        } else {
            None
        }
    });

    assert!(impact.is_some(), "expected ApprovalRequired event");
    let impact = impact.unwrap();
    assert!(
        !impact.is_empty(),
        "expected non-empty impact for file with known importer"
    );
    assert!(
        impact.contains(&"src/importer.rs".to_string()),
        "impact must list the importer file; got: {impact:?}"
    );
}

#[test]
fn edit_file_approval_has_empty_impact_when_no_importers() {
    use rusqlite::Connection;
    use tempfile::{NamedTempFile, TempDir};

    use crate::runtime::ProjectRoot;
    use crate::storage::index::SymbolStore;
    use crate::storage::session::schema;

    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/isolated.rs"), "fn foo() {}\n").unwrap();

    let db_file = NamedTempFile::new().unwrap();
    {
        let conn = Connection::open(db_file.path()).unwrap();
        schema::initialize(&conn).unwrap();
    }
    // No edges seeded — isolated.rs has no importers.
    drop(SymbolStore::open(db_file.path()).unwrap());

    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(TestBackend::new(vec![
            "[edit_file]\npath: src/isolated.rs\n---search---\nfn foo() {}\n---replace---\nfn bar() {}\n[/edit_file]",
        ])),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        tmp.path().to_path_buf(),
        "test-no-impact".to_string(),
    )
    .with_symbol_store(db_file.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "replace foo with bar in src/isolated.rs".into(),
        },
    );

    let impact = events.iter().find_map(|e| {
        if let RuntimeEvent::ApprovalRequired { impact, .. } = e {
            Some(impact.clone())
        } else {
            None
        }
    });

    assert!(impact.is_some(), "expected ApprovalRequired event");
    assert!(
        impact.unwrap().is_empty(),
        "expected empty impact for file with no importers"
    );
}
