use std::collections::HashSet;
use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::core::config::LspConfig;
use crate::runtime::investigation::anchors::AnchorState;
use crate::runtime::investigation::investigation::{InvestigationMode, InvestigationState};
use crate::runtime::investigation::tool_surface::ToolSurface;
use crate::runtime::lsp::LspManager;
use crate::runtime::orchestration::tool_round::{run_tool_round, SearchBudget, ToolRoundOutcome};
use crate::tools::{default_registry, ToolInput, ToolRegistry};

fn temp_root() -> (TempDir, ProjectRoot, ToolRegistry) {
    let dir = TempDir::new().unwrap();
    let root = ProjectRoot::new(dir.path().to_path_buf()).unwrap();
    let registry = default_registry().with_project_root(root.as_path_buf());
    (dir, root, registry)
}

fn run_round(
    root: &ProjectRoot,
    registry: &ToolRegistry,
    calls: Vec<ToolInput>,
    tool_surface: ToolSurface,
    investigation_required: bool,
) -> ToolRoundOutcome {
    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut lsp = LspManager::new(&LspConfig::default(), Path::new("."));
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed = 0usize;
    let mut weak_query = 0usize;

    run_tool_round(
        root,
        registry,
        calls,
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut lsp,
        &mut reads_this_turn,
        &mut anchors,
        tool_surface,
        &mut disallowed,
        &mut weak_query,
        false,
        investigation_required,
        InvestigationMode::General,
        None,
        &mut requested_read_completed,
        None,
        None,
        &mut |_| {},
    )
}

// 1. Regression for Phase 29.5: scope pointing to a file, not a directory.
#[test]
fn search_code_with_file_scope_uses_parent_directory() {
    // Prompt scope extracts to "src/foo.rs" (a file). resolve_scope must fall back
    // to the parent directory "src/" and return search results, not a tool error.
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(
        tmp.path().join("src/foo.rs"),
        "pub fn foo_scope_29_7_unique() {}\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: foo_scope_29_7_unique]",
            "[read_file: src/foo.rs]",
            "foo_scope_29_7_unique is in src/foo.rs.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is foo_scope_29_7_unique defined in src/foo.rs".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "file-scoped search must not fail: {events:?}"
    );

    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: search_code ===")),
        "search must execute and return results, not a resolution error"
    );
    assert!(
        !snapshot.iter().any(|m| {
            m.content.contains("=== tool_error: search_code ===")
                && m.content.contains("not a directory")
        }),
        "file-scoped search must not produce a not-a-directory error"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "file-scoped search must complete as ToolAssisted: {answer_source:?}"
    );
}

// 2. Directory scope succeeds (baseline confirming existing behavior is preserved).
#[test]
fn search_code_with_directory_scope_succeeds() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(
        tmp.path().join("src/foo.rs"),
        "pub fn foo_scope_29_7_unique() {}\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: foo_scope_29_7_unique]",
            "[read_file: src/foo.rs]",
            "foo_scope_29_7_unique is in src/foo.rs.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is foo_scope_29_7_unique defined in src/".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "directory-scoped search must not fail: {events:?}"
    );
    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: search_code ===")),
        "directory-scoped search must return results"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "directory-scoped search must complete as ToolAssisted: {answer_source:?}"
    );
}

// 3. list_dir returns real directory entries from a temp directory.
#[test]
fn list_dir_succeeds_on_real_directory() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("alpha.rs"), "fn alpha() {}\n").unwrap();
    fs::write(tmp.path().join("beta.rs"), "fn beta() {}\n").unwrap();
    fs::write(tmp.path().join("gamma.rs"), "fn gamma() {}\n").unwrap();

    let mut rt = make_runtime_in(
        vec!["[list_dir: .]", "The directory has alpha, beta, and gamma."],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "display the structure".into(),
        },
    );

    assert!(!has_failed(&events), "list_dir must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    let list_result = snapshot
        .iter()
        .find(|m| m.content.contains("=== tool_result: list_dir ==="))
        .map(|m| m.content.as_str())
        .unwrap_or("");
    assert!(
        !list_result.is_empty(),
        "list_dir must produce a result block"
    );
    assert!(
        list_result.contains("alpha.rs")
            || list_result.contains("beta.rs")
            || list_result.contains("gamma.rs"),
        "list_dir result must include real files: {list_result}"
    );
}

// 4. DefinitionLookup with real search seeds lsp_definition at the declaration line.
#[test]
fn lsp_definition_seeded_on_definition_lookup_with_real_search() {
    // Line 1 is a comment mentioning MyStruct; line 3 is the struct declaration.
    // The seeded lsp_definition must target line 3, not line 1.
    // LSP is enabled so seeding fires; we only run one round and check the dispatch
    // outcome — the actual LSP server call never happens.
    let (dir, root, registry) = temp_root();
    fs::write(
        dir.path().join("mymodule.rs"),
        "// MyStruct29_7 holds the state\n\npub struct MyStruct29_7 {\n    value: i32,\n}\n",
    )
    .unwrap();

    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut lsp = LspManager::new(
        &LspConfig {
            enabled: true,
            ..LspConfig::default()
        },
        root.path(),
    );
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed = 0usize;
    let mut weak_query = 0usize;

    let outcome = run_tool_round(
        &root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "MyStruct29_7".into(),
            path: None,
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut lsp,
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed,
        &mut weak_query,
        false,
        true,
        InvestigationMode::DefinitionLookup,
        None,
        &mut requested_read_completed,
        None,
        None,
        &mut |_| {},
    );

    let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
        panic!("DefinitionLookup after real search must seed lsp_definition (RuntimeDispatch)");
    };
    let ToolInput::LspDefinition { path, line, col } = call else {
        panic!("dispatched call must be lsp_definition, got: {call:?}");
    };
    assert_eq!(
        path, "mymodule.rs",
        "lsp_definition must target the definition candidate"
    );
    assert_eq!(
        line, 3,
        "lsp_definition must use declaration line (3), not comment line (1): line={line}"
    );
    assert!(col >= 1, "column must be 1-based: col={col}");
}

// 5. Non-candidate read after real search dispatches to the candidate, not a tool error.
#[test]
fn non_candidate_read_redirects_to_candidate_with_real_files() {
    let (dir, root, registry) = temp_root();
    fs::write(
        dir.path().join("candidate.rs"),
        "fn needle_29_7_unique() {}\n",
    )
    .unwrap();
    fs::write(dir.path().join("other.rs"), "fn unrelated() {}\n").unwrap();

    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed = 0usize;
    let mut weak_query = 0usize;

    // Round 1: search populates candidate list with candidate.rs.
    run_tool_round(
        &root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "needle_29_7_unique".into(),
            path: None,
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut LspManager::new(&LspConfig::default(), root.path()),
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed,
        &mut weak_query,
        false,
        true,
        InvestigationMode::General,
        None,
        &mut requested_read_completed,
        None,
        None,
        &mut |_| {},
    );

    assert!(
        investigation.search_produced_results(),
        "search must have found candidate.rs"
    );

    // Round 2: model reads other.rs (not a candidate) — runtime dispatches candidate.rs.
    let outcome = run_tool_round(
        &root,
        &registry,
        vec![ToolInput::ReadFile {
            path: "other.rs".into(),
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut LspManager::new(&LspConfig::default(), root.path()),
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed,
        &mut weak_query,
        false,
        true,
        InvestigationMode::General,
        None,
        &mut requested_read_completed,
        None,
        None,
        &mut |_| {},
    );

    let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
        panic!("non-candidate read must dispatch the preferred candidate (RuntimeDispatch)");
    };
    let ToolInput::ReadFile { path } = call else {
        panic!("dispatched call must be read_file, got: {call:?}");
    };
    assert_eq!(
        path, "candidate.rs",
        "dispatch must target the preferred candidate"
    );
}

// 6. Resolver rejects paths that escape the project root via ../.
#[test]
fn resolver_rejects_path_outside_project_root() {
    let (dir, root, registry) = temp_root();
    let outside_name = format!(
        "outside-{}.txt",
        dir.path().file_name().unwrap().to_string_lossy()
    );
    let outside_file = dir.path().parent().unwrap().join(&outside_name);
    fs::write(&outside_file, "secret\n").unwrap();

    let outcome = run_round(
        &root,
        &registry,
        vec![ToolInput::ReadFile {
            path: format!("../{outside_name}"),
        }],
        ToolSurface::RetrievalFirst,
        false,
    );

    fs::remove_file(outside_file).unwrap();

    let ToolRoundOutcome::TerminalAnswer { results, .. } = outcome else {
        panic!("path escape must produce a TerminalAnswer");
    };
    assert!(
        results.contains("=== tool_error: read_file ==="),
        "resolver rejection must produce a tool_error block: {results}"
    );
    assert!(
        results.contains("escapes project root"),
        "error message must mention root escape: {results}"
    );
}

// 7. DefinitionLookup: truncated results with no declaration dispatches refined "fn {query}" search.
#[test]
fn definition_lookup_truncated_no_declaration_dispatches_refinement() {
    // Create 6 files × 3 usage lines each = 18 matches, exceeding MAX_RESULTS_SHOWN (15).
    // None of the lines contains a declaration, so first_definition_candidate() returns None.
    // The runtime must dispatch RuntimeDispatch::SearchCode with query "fn process_29_15".
    let (dir, root, registry) = temp_root();
    for i in 0..6usize {
        let filename = format!("worker_{i}.rs");
        let content = format!(
            "let _ = process_29_15(job_{i}_a);\nlet _ = process_29_15(job_{i}_b);\nlet _ = process_29_15(job_{i}_c);\n"
        );
        fs::write(dir.path().join(&filename), &content).unwrap();
    }

    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut lsp = LspManager::new(&LspConfig::default(), root.path());
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed = 0usize;
    let mut weak_query = 0usize;

    let outcome = run_tool_round(
        &root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "process_29_15".into(),
            path: None,
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut lsp,
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed,
        &mut weak_query,
        false,
        true,
        InvestigationMode::DefinitionLookup,
        None,
        &mut requested_read_completed,
        None,
        None,
        &mut |_| {},
    );

    let ToolRoundOutcome::RuntimeDispatch { call, .. } = outcome else {
        panic!(
            "truncated DefinitionLookup with no declaration must dispatch refinement (RuntimeDispatch)"
        );
    };
    let ToolInput::SearchCode { query, .. } = call else {
        panic!("dispatched call must be search_code, got: {call:?}");
    };
    assert!(
        query.starts_with("fn "),
        "refined query must start with 'fn ', got: {query:?}"
    );
    assert!(
        investigation.definition_refinement_issued(),
        "definition_refinement_issued must be true after dispatch"
    );
}

// 8. search_code with a nonexistent scope path fails gracefully (no panic).
#[test]
fn search_code_with_nonexistent_scope_path_fails_gracefully() {
    let (_dir, root, registry) = temp_root();

    let outcome = run_round(
        &root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "anything".into(),
            path: Some("nonexistent_scope_29_7/".into()),
        }],
        ToolSurface::RetrievalFirst,
        false,
    );

    let ToolRoundOutcome::Completed { results, .. } = outcome else {
        panic!("nonexistent scope must produce Completed with a tool error");
    };
    assert!(
        results.contains("=== tool_error: search_code ==="),
        "nonexistent scope must produce a tool_error block: {results}"
    );
    assert!(
        results.contains("invalid tool input:"),
        "error must be an invalid-input tool error: {results}"
    );
}

// 9. Slice 30.3: index hit on DefinitionLookup promotes candidate into
// definition_site_candidates so it wins over usage-only rg results.
#[test]
fn index_hit_promotes_definition_candidate_on_definition_lookup() {
    use crate::storage::index::types::{ExtractedSymbol, SymbolConfidence, SymbolKind};
    use crate::storage::index::SymbolStore;
    use crate::storage::session::SessionStore;

    let (dir, root, registry) = temp_root();

    // A file that has a usage but not a definition — rg will find it but
    // it won't become a definition_site_candidate from record_search_results.
    fs::write(dir.path().join("usage_30_3.rs"), "let _ = my_fn_30_3(x);\n").unwrap();

    // Initialize schema via SessionStore (SymbolStore::open does not init schema).
    let db_path = dir.path().join("thunk_30_3.db");
    SessionStore::open(&db_path).unwrap();
    let store = SymbolStore::open(&db_path).unwrap();
    let root_str = root.path().to_string_lossy().to_string();
    store
        .upsert_symbols(
            &root_str,
            &[ExtractedSymbol {
                name: "my_fn_30_3".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/impl_30_3.rs".to_string(),
                line: 5,
                col: 1,
                signature: "pub fn my_fn_30_3()".to_string(),
                confidence: SymbolConfidence::High,
            }],
        )
        .unwrap();

    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut lsp = LspManager::new(&LspConfig::default(), root.path());
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed = 0usize;
    let mut weak_query = 0usize;

    run_tool_round(
        &root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "my_fn_30_3".into(),
            path: None,
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut lsp,
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed,
        &mut weak_query,
        false,
        true,
        InvestigationMode::DefinitionLookup,
        None,
        &mut requested_read_completed,
        None,
        Some(&store),
        &mut |_| {},
    );

    assert!(
        investigation.search_produced_results(),
        "rg must find usage_30_3.rs"
    );
    assert_eq!(
        investigation.first_definition_candidate(),
        Some("src/impl_30_3.rs"),
        "index-promoted path must be the first definition candidate"
    );
}

// 10. Slice 30.5: import edges from the symbol index pre-seed the
// InvestigationGraph at turn start so promoted_candidates can surface
// index-sourced relations without requiring runtime file reads.
#[test]
fn import_edges_from_index_pre_seed_investigation_graph() {
    use crate::storage::index::types::ImportEdge;
    use crate::storage::index::SymbolStore;
    use crate::storage::session::SessionStore;

    let (dir, root, _registry) = temp_root();

    let db_path = dir.path().join("thunk_30_5.db");
    SessionStore::open(&db_path).unwrap();
    let store = SymbolStore::open(&db_path).unwrap();
    let root_str = root.path().to_string_lossy().to_string();

    store
        .upsert_imports(
            &root_str,
            &[ImportEdge {
                from_file: "src/main.py".to_string(),
                to_file: "models/task.py".to_string(),
            }],
        )
        .unwrap();

    // Apply the same pre-seeding logic as run_turns_with_initial_reads.
    let mut investigation = InvestigationState::new();
    if store.import_count(&root_str).unwrap_or(0) > 0 {
        if let Ok(edges) = store.all_imports(&root_str) {
            for edge in &edges {
                investigation
                    .graph
                    .record_import_edge(&edge.from_file, &edge.to_file);
            }
        }
    }

    // Simulate a read of src/main.py with no content — edges are already
    // pre-seeded, so the graph only needs the node marked as read.
    investigation.graph.record_read("src/main.py", "");

    let promoted = investigation.graph.promoted_candidates();
    assert!(
        promoted.contains(&"models/task.py".to_string()),
        "index-pre-seeded import edge must promote candidate after source is read; got {promoted:?}"
    );
}
