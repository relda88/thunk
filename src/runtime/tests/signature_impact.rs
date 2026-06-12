use rusqlite::Connection;
use tempfile::{NamedTempFile, TempDir};

use super::*;
use crate::core::config::Config;
use crate::runtime::ProjectRoot;
use crate::storage::index::types::{ExtractedSymbol, ImportEdge, SymbolConfidence, SymbolKind};
use crate::storage::index::SymbolStore;
use crate::storage::session::schema;

/// Helper: initialize an in-memory-adjacent DB file with the schema and seed symbols + imports.
fn seed_store(
    db_path: &std::path::Path,
    root_str: &str,
    file_rel: &str,
    symbols: &[(&str, &str)], // (name, signature)
    importers: &[&str],
) {
    let conn = Connection::open(db_path).unwrap();
    schema::initialize(&conn).unwrap();
    drop(conn);

    let store = SymbolStore::open(db_path).unwrap();
    let syms: Vec<ExtractedSymbol> = symbols
        .iter()
        .enumerate()
        .map(|(i, (name, sig))| ExtractedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: file_rel.to_string(),
            line: i + 1,
            col: 1,
            signature: sig.to_string(),
            confidence: SymbolConfidence::High,
            parent_scope: None,
        })
        .collect();
    if !syms.is_empty() {
        store
            .upsert_symbols_for_file(root_str, file_rel, &syms)
            .unwrap();
    }
    let edges: Vec<ImportEdge> = importers
        .iter()
        .map(|imp| ImportEdge {
            from_file: imp.to_string(),
            to_file: file_rel.to_string(),
        })
        .collect();
    if !edges.is_empty() {
        store.upsert_imports(root_str, &edges).unwrap();
    }
}

/// Collects all SystemMessage texts from a slice of events.
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

/// When LSP is disabled (default test config), a signature change after an approved
/// edit falls back to the diff_msg text rather than "call sites:".
#[test]
fn signature_change_without_lsp_emits_diff_fallback_not_call_sites() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();

    let file_content = "pub fn process() -> i32 { 42 }\n";
    std::fs::write(tmp.path().join("src/lib.rs"), file_content).unwrap();

    let db = NamedTempFile::new().unwrap();
    let canonical_root = tmp.path().canonicalize().unwrap();
    let root_str = canonical_root.to_string_lossy().to_string();

    seed_store(
        db.path(),
        &root_str,
        "src/lib.rs",
        &[("process", "pub fn process() -> i32 { 42 }")],
        &["src/caller.rs"],
    );

    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(TestBackend::new(vec![
            "[edit_file]\npath: src/lib.rs\n---search---\npub fn process() -> i32 { 42 }\n---replace---\npub fn process(n: i32) -> i32 { n }\n[/edit_file]",
        ])),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        tmp.path().to_path_buf(),
        "test-sig-impact".to_string(),
    )
    .with_symbol_store(db.path());

    // Submit → get approval.
    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "change process to take n: i32".into(),
        },
    );
    assert!(
        has_approval(&submit_events),
        "expected ApprovalRequired; got: {submit_events:?}"
    );

    // Approve → execute mutation.
    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve must not fail; got: {approve_events:?}"
    );

    // Verify: no "call sites:" in any SystemMessage — LSP was not running.
    let msgs = system_messages(&approve_events);
    for msg in &msgs {
        assert!(
            !msg.contains("call sites:"),
            "LSP disabled — must not emit call-site list; got: {msg:?}"
        );
    }
}

/// When after_recs contains no matching symbol (all-removed case), changed_names is
/// empty so no LSP call is attempted and no panic occurs.
#[test]
fn signature_diff_with_removed_symbol_does_not_panic() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();

    // File has original; mutation replaces it with a comment the indexer won't parse.
    let file_content = "pub fn original() {}\n";
    std::fs::write(tmp.path().join("src/target.rs"), file_content).unwrap();

    let db = NamedTempFile::new().unwrap();
    let canonical_root = tmp.path().canonicalize().unwrap();
    let root_str = canonical_root.to_string_lossy().to_string();

    // Seed a symbol so is_empty() returns false (allows rebuild to run).
    seed_store(
        db.path(),
        &root_str,
        "src/target.rs",
        &[("original", "pub fn original() {}")],
        &[],
    );

    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(TestBackend::new(vec![
            // Replace with a comment — indexer will find no symbols in after-state.
            "[edit_file]\npath: src/target.rs\n---search---\npub fn original() {}\n---replace---\n// removed\n[/edit_file]",
        ])),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        tmp.path().to_path_buf(),
        "test-removed-sym".to_string(),
    )
    .with_symbol_store(db.path());

    // Use a mutation-enabling prompt (must not trigger requested_simple_edit).
    let submit_events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "update src/target.rs to replace original with a comment".into(),
        },
    );
    assert!(
        has_approval(&submit_events),
        "expected ApprovalRequired; got: {submit_events:?}"
    );

    // Approve — changed_names will be empty (only Removed, not Changed), so
    // no LSP references call is attempted and the code must not panic.
    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(
        !has_failed(&approve_events),
        "approve must not fail; got: {approve_events:?}"
    );
}
