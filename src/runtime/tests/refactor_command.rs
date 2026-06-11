use rusqlite::Connection;

use crate::storage::session::schema;
use crate::storage::tasks::EditSequenceStore;

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
