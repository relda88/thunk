use rusqlite::Connection;
use tempfile::NamedTempFile;

use crate::runtime::RuntimeRequest;
use crate::storage::retrieval::RetrievalLogStore;
use crate::storage::session::schema;

use super::*;

fn open_retrieval_store(path: &std::path::Path) -> RetrievalLogStore {
    let conn = Connection::open(path).unwrap();
    schema::initialize(&conn).unwrap();
    drop(conn);
    RetrievalLogStore::open(path).unwrap()
}

fn canonical_root() -> String {
    std::path::PathBuf::from(".")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

#[test]
fn retrieval_log_entry_written_after_submit_turn() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_path_buf();

    let root = ProjectRoot::new(std::path::PathBuf::from(".")).unwrap();
    let mut runtime = Runtime::new(
        &crate::core::config::Config::default(),
        root.clone(),
        Box::new(TestBackend::new(vec!["The answer is 42."])),
        crate::tools::default_registry().with_project_root(root.as_path_buf()),
        None,
        std::path::PathBuf::from("/tmp"),
        "test-session".to_string(),
    )
    .with_retrieval_log_store(&db_path);

    let mut events = Vec::new();
    runtime.handle(
        RuntimeRequest::Submit {
            text: "what is the answer?".to_string(),
        },
        &mut |e| events.push(e),
    );

    let store = open_retrieval_store(&db_path);
    let project_root = canonical_root();
    let entries = store.last_n(&project_root, 10).unwrap();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one log entry after submit"
    );
}

#[test]
fn retrieval_log_command_shows_entries() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_path_buf();

    let root = ProjectRoot::new(std::path::PathBuf::from(".")).unwrap();
    let mut runtime = Runtime::new(
        &crate::core::config::Config::default(),
        root.clone(),
        Box::new(TestBackend::new(vec![
            "First answer.",
            "Second answer.",
            "retrieval log (last 2):",
        ])),
        crate::tools::default_registry().with_project_root(root.as_path_buf()),
        None,
        std::path::PathBuf::from("/tmp"),
        "test-session".to_string(),
    )
    .with_retrieval_log_store(&db_path);

    collect_events(
        &mut runtime,
        RuntimeRequest::Submit {
            text: "q1".to_string(),
        },
    );
    collect_events(
        &mut runtime,
        RuntimeRequest::Submit {
            text: "q2".to_string(),
        },
    );

    let events = collect_events(&mut runtime, RuntimeRequest::RetrievalLog { n: Some(2) });
    let has_log_output = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("retrieval log")));
    assert!(has_log_output, "expected a retrieval log SystemMessage");
}

#[test]
fn retrieval_log_no_entries_message_when_empty() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_path_buf();

    let root = ProjectRoot::new(std::path::PathBuf::from(".")).unwrap();
    let mut runtime = Runtime::new(
        &crate::core::config::Config::default(),
        root.clone(),
        Box::new(TestBackend::new(Vec::<&str>::new())),
        crate::tools::default_registry().with_project_root(root.as_path_buf()),
        None,
        std::path::PathBuf::from("/tmp"),
        "test-session".to_string(),
    )
    .with_retrieval_log_store(&db_path);

    let events = collect_events(&mut runtime, RuntimeRequest::RetrievalLog { n: None });
    let has_empty = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("no entries")));
    assert!(has_empty, "expected 'no entries' message for empty log");
}
