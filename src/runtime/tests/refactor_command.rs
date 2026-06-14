use rusqlite::Connection;

use crate::storage::session::schema;
use crate::storage::tasks::{
    EditSequence, EditSequenceStore, EditStep, SequenceStatus, StepStatus,
};

use super::*;

fn attach_edit_store(runtime: &mut Runtime, path: &std::path::Path) {
    let init_conn = Connection::open(path).unwrap();
    schema::initialize(&init_conn).unwrap();
    drop(init_conn);
    let store = EditSequenceStore::open(path).unwrap();
    runtime.edit_store = Some(store);
}

#[test]
fn refactor_with_valid_json_persists_sequence_and_emits_message() {
    let json = r#"[{"file":"src/foo.rs","description":"extract helper"},{"file":"src/bar.rs","description":"update callsites"}]"#;
    let mut runtime = make_runtime(vec![json]);
    let tmp = tempfile::NamedTempFile::new().unwrap();
    attach_edit_store(&mut runtime, tmp.path());

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::Refactor {
            target: Some("extract error handling".to_string()),
        },
    );

    let system_msg = events.iter().find(
        |e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("Refactor sequence created")),
    );
    assert!(
        system_msg.is_some(),
        "expected SystemMessage with sequence summary, got: {events:?}"
    );

    let conn = Connection::open(tmp.path()).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edit_sequences WHERE status = 'planning'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "one sequence row should be persisted");

    let step_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM edit_steps", [], |row| row.get(0))
        .unwrap();
    assert_eq!(step_count, 2, "two step rows should be persisted");
}

#[test]
fn refactor_with_malformed_json_emits_failed_with_raw_output() {
    let bad_response = "sorry I cannot do that right now";
    let mut runtime = make_runtime(vec![bad_response]);
    let tmp = tempfile::NamedTempFile::new().unwrap();
    attach_edit_store(&mut runtime, tmp.path());

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::Refactor {
            target: Some("cleanup imports".to_string()),
        },
    );

    let failed = events.iter().find_map(|e| {
        if let RuntimeEvent::Failed { message } = e {
            Some(message.as_str())
        } else {
            None
        }
    });
    assert!(failed.is_some(), "expected Failed event");
    assert!(
        failed.unwrap().contains(bad_response),
        "Failed message should contain raw model output"
    );
}

#[test]
fn refactor_without_edit_store_emits_failed() {
    let mut runtime = make_runtime(vec!["[]"]);
    // no edit_store attached

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::Refactor {
            target: Some("some goal".to_string()),
        },
    );

    let failed = events.iter().find_map(|e| {
        if let RuntimeEvent::Failed { message } = e {
            Some(message.as_str())
        } else {
            None
        }
    });
    assert!(
        failed.is_some(),
        "expected Failed event when edit_store is None"
    );
    assert!(
        failed.unwrap().contains("Edit store not initialized"),
        "message should indicate store is absent"
    );
}

fn make_sequence_with_step(
    seq_id: &str,
    step_id: &str,
    file: &std::path::Path,
    search: &str,
    replace: &str,
) -> EditSequence {
    EditSequence {
        id: seq_id.to_string(),
        task_id: None,
        goal: "test refactor".to_string(),
        steps: vec![EditStep {
            id: step_id.to_string(),
            sequence_id: seq_id.to_string(),
            position: 0,
            file: file.to_path_buf(),
            search: search.to_string(),
            replace: replace.to_string(),
            verification_cmd: None,
            status: StepStatus::Pending,
            generated: false,
        }],
        current_idx: 0,
        status: SequenceStatus::Approved,
        snapshot_ref: None,
    }
}

#[test]
fn sequence_execute_step_applies_patch_and_advances() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let target_file = tmp_dir.path().join("target.rs");
    std::fs::write(&target_file, "fn old_fn() {}\n").unwrap();

    let db = tmp_dir.path().join("store.db");
    let mut runtime = make_runtime_in(Vec::<&str>::new(), tmp_dir.path());
    attach_edit_store(&mut runtime, &db);

    // search must match the exact line content so mpatch can anchor the hunk
    let seq = make_sequence_with_step(
        "seq1",
        "step1",
        &target_file,
        "fn old_fn() {}",
        "fn new_fn() {}",
    );
    runtime
        .edit_store
        .as_ref()
        .unwrap()
        .create_sequence(&seq)
        .unwrap();
    runtime.active_sequence_id = Some("seq1".to_string());

    let events = collect_events(&mut runtime, RuntimeRequest::SequenceExecuteStep);

    let sys_msg = events.iter().find(
        |e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("applied and verified")),
    );
    assert!(
        sys_msg.is_some(),
        "expected step applied message; got: {events:?}"
    );

    let patched = std::fs::read_to_string(&target_file).unwrap();
    assert!(patched.contains("fn new_fn() {}"), "file should be patched");

    // Pointer must have advanced — get_current_step returns None (sequence done).
    let step = runtime
        .edit_store
        .as_ref()
        .unwrap()
        .get_current_step("seq1")
        .unwrap();
    assert!(
        step.is_none(),
        "step pointer should have advanced past last step"
    );
}

#[test]
fn sequence_execute_step_anchor_not_found_emits_failed_and_aborts() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let target_file = tmp_dir.path().join("target.rs");
    std::fs::write(&target_file, "fn unrelated() {}\n").unwrap();

    let db = tmp_dir.path().join("store.db");
    let mut runtime = make_runtime_in(Vec::<&str>::new(), tmp_dir.path());
    attach_edit_store(&mut runtime, &db);

    // search text that does NOT exist in the file
    let seq = make_sequence_with_step(
        "seq2",
        "step2",
        &target_file,
        "fn does_not_exist()",
        "fn x()",
    );
    runtime
        .edit_store
        .as_ref()
        .unwrap()
        .create_sequence(&seq)
        .unwrap();
    runtime.active_sequence_id = Some("seq2".to_string());

    let events = collect_events(&mut runtime, RuntimeRequest::SequenceExecuteStep);

    let failed = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::Failed { .. }));
    assert!(failed.is_some(), "expected Failed event; got: {events:?}");

    // active_sequence_id must be cleared after abort
    assert!(
        runtime.active_sequence_id.is_none(),
        "active_sequence_id should be cleared after abort"
    );

    // sequence status must be Failed in the store
    let seq_loaded = runtime
        .edit_store
        .as_ref()
        .unwrap()
        .get_sequence("seq2")
        .unwrap()
        .unwrap();
    assert_eq!(seq_loaded.status, SequenceStatus::Failed);
}

#[test]
fn sequence_abort_clears_state_and_sets_failed() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let db = tmp_dir.path().join("store.db");
    let mut runtime = make_runtime_in(Vec::<&str>::new(), tmp_dir.path());
    attach_edit_store(&mut runtime, &db);

    let seq = EditSequence {
        id: "seqX".to_string(),
        task_id: None,
        goal: "goal".to_string(),
        steps: vec![],
        current_idx: 0,
        status: SequenceStatus::Approved,
        snapshot_ref: None,
    };
    runtime
        .edit_store
        .as_ref()
        .unwrap()
        .create_sequence(&seq)
        .unwrap();
    runtime.active_sequence_id = Some("seqX".to_string());

    let events = collect_events(&mut runtime, RuntimeRequest::SequenceAbort);

    assert!(
        runtime.active_sequence_id.is_none(),
        "active_sequence_id should be cleared"
    );

    // New message indicates no snapshot was available (snapshot_ref was None).
    let sys = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("aborted")));
    assert!(sys.is_some(), "expected aborted message; got: {events:?}");

    let loaded = runtime
        .edit_store
        .as_ref()
        .unwrap()
        .get_sequence("seqX")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, SequenceStatus::Failed);
}

fn make_two_step_sequence(
    seq_id: &str,
    file1: &std::path::Path,
    file2: &std::path::Path,
) -> EditSequence {
    EditSequence {
        id: seq_id.to_string(),
        task_id: None,
        goal: "two-step refactor".to_string(),
        steps: vec![
            EditStep {
                id: format!("{seq_id}_s0"),
                sequence_id: seq_id.to_string(),
                position: 0,
                file: file1.to_path_buf(),
                search: "fn step_zero() {}".to_string(),
                replace: "fn step_zero_new() {}".to_string(),
                verification_cmd: None,
                status: StepStatus::Pending,
                generated: false,
            },
            EditStep {
                id: format!("{seq_id}_s1"),
                sequence_id: seq_id.to_string(),
                position: 1,
                file: file2.to_path_buf(),
                search: "fn step_one() {}".to_string(),
                replace: "fn step_one_new() {}".to_string(),
                verification_cmd: None,
                status: StepStatus::Pending,
                generated: false,
            },
        ],
        current_idx: 0,
        status: SequenceStatus::Approved,
        snapshot_ref: None,
    }
}

#[test]
fn sequence_execute_step_auto_advances_through_all_steps() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let file1 = tmp_dir.path().join("a.rs");
    let file2 = tmp_dir.path().join("b.rs");
    std::fs::write(&file1, "fn step_zero() {}\n").unwrap();
    std::fs::write(&file2, "fn step_one() {}\n").unwrap();

    let db = tmp_dir.path().join("store.db");
    let mut runtime = make_runtime_in(Vec::<&str>::new(), tmp_dir.path());
    attach_edit_store(&mut runtime, &db);

    let seq = make_two_step_sequence("seq_auto", &file1, &file2);
    runtime
        .edit_store
        .as_ref()
        .unwrap()
        .create_sequence(&seq)
        .unwrap();
    runtime.active_sequence_id = Some("seq_auto".to_string());

    // Single call to handle_sequence_execute_step must run both steps to completion.
    let events = collect_events(&mut runtime, RuntimeRequest::SequenceExecuteStep);

    let step0_msg = events.iter().find(
        |e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("Step 0 applied and verified")),
    );
    assert!(
        step0_msg.is_some(),
        "expected step 0 applied message; got: {events:?}"
    );

    let step1_msg = events.iter().find(
        |e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("Step 1 applied and verified")),
    );
    assert!(
        step1_msg.is_some(),
        "expected step 1 applied message; got: {events:?}"
    );

    let complete_msg = events.iter().find(
        |e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("Refactor sequence complete")),
    );
    assert!(
        complete_msg.is_some(),
        "expected completion message; got: {events:?}"
    );

    assert!(
        runtime.active_sequence_id.is_none(),
        "active_sequence_id should be None after completion"
    );

    assert!(
        std::fs::read_to_string(&file1)
            .unwrap()
            .contains("fn step_zero_new()"),
        "file1 should be patched"
    );
    assert!(
        std::fs::read_to_string(&file2)
            .unwrap()
            .contains("fn step_one_new()"),
        "file2 should be patched"
    );

    let seq_loaded = runtime
        .edit_store
        .as_ref()
        .unwrap()
        .get_sequence("seq_auto")
        .unwrap()
        .unwrap();
    assert_eq!(seq_loaded.status, SequenceStatus::Completed);
}

#[test]
fn sequence_mid_run_step_failure_aborts_and_clears_state() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let file1 = tmp_dir.path().join("good.rs");
    let file2 = tmp_dir.path().join("bad.rs");
    // file1 has the correct search text; file2 does NOT have the step 1 search text.
    std::fs::write(&file1, "fn step_zero() {}\n").unwrap();
    std::fs::write(&file2, "fn unrelated() {}\n").unwrap();

    let db = tmp_dir.path().join("store.db");
    let mut runtime = make_runtime_in(Vec::<&str>::new(), tmp_dir.path());
    attach_edit_store(&mut runtime, &db);

    let seq = make_two_step_sequence("seq_fail", &file1, &file2);
    runtime
        .edit_store
        .as_ref()
        .unwrap()
        .create_sequence(&seq)
        .unwrap();
    runtime.active_sequence_id = Some("seq_fail".to_string());

    let events = collect_events(&mut runtime, RuntimeRequest::SequenceExecuteStep);

    // Step 0 succeeds, step 1 fails (anchor not found in file2).
    let step0_ok = events.iter().any(
        |e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("Step 0 applied and verified")),
    );
    assert!(step0_ok, "step 0 should have succeeded; got: {events:?}");

    let failed = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::Failed { .. }));
    assert!(
        failed.is_some(),
        "expected Failed event for step 1; got: {events:?}"
    );

    assert!(
        runtime.active_sequence_id.is_none(),
        "active_sequence_id should be None after abort"
    );

    let seq_loaded = runtime
        .edit_store
        .as_ref()
        .unwrap()
        .get_sequence("seq_fail")
        .unwrap()
        .unwrap();
    assert_eq!(
        seq_loaded.status,
        SequenceStatus::Failed,
        "sequence status should be Failed"
    );

    // Abort message must contain "aborted" (no snapshot → "no snapshot available" path).
    let abort_msg = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("aborted")));
    assert!(
        abort_msg.is_some(),
        "expected abort SystemMessage; got: {events:?}"
    );
}
