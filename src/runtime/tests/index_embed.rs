use std::sync::Arc;

use rusqlite::Connection;
use tempfile::NamedTempFile;

use crate::core::error::{AppError, Result};
use crate::runtime::index::EmbeddingProvider;
use crate::runtime::index::ExtractedSymbol;
use crate::runtime::RuntimeRequest;
use crate::runtime::{SymbolConfidence, SymbolKind};
use crate::storage::index::SymbolStore;
use crate::storage::session::schema;

use super::*;

// Mock embedding provider

struct ConstantEmbedProvider(Vec<f32>);

impl EmbeddingProvider for ConstantEmbedProvider {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| self.0.clone()).collect())
    }
}

struct FailingEmbedProvider;

impl EmbeddingProvider for FailingEmbedProvider {
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Err(AppError::Runtime("embed: provider unavailable".to_string()))
    }
}

// Helpers

fn open_store(path: &std::path::Path) -> SymbolStore {
    // Initialize schema first, then open via the normal API.
    let conn = Connection::open(path).unwrap();
    schema::initialize(&conn).unwrap();
    drop(conn);
    SymbolStore::open(path).unwrap()
}

fn canonical_root() -> String {
    std::path::PathBuf::from(".")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

fn seed_symbols(store: &SymbolStore, root: &str, names: &[&str]) {
    let syms: Vec<ExtractedSymbol> = names
        .iter()
        .map(|n| ExtractedSymbol {
            name: n.to_string(),
            kind: SymbolKind::Function,
            file_path: format!("src/{n}.rs"),
            line: 1,
            col: 1,
            signature: format!("pub fn {n}()"),
            confidence: SymbolConfidence::High,
            parent_scope: None,
        })
        .collect();
    store.upsert_symbols(root, &syms).unwrap();
}

fn make_runtime_with_store_and_provider(
    db_path: &std::path::Path,
    provider: Arc<dyn EmbeddingProvider + Send + Sync>,
) -> (Runtime, String) {
    use crate::core::config::Config;
    use crate::runtime::ProjectRoot;
    use crate::tools::default_registry;
    use std::path::PathBuf;

    let root = ProjectRoot::new(PathBuf::from(".")).unwrap();
    let root_str = root.path().to_string_lossy().to_string();
    let rt = Runtime::new(
        &Config::default(),
        root.clone(),
        Box::new(TestBackend::new(Vec::<String>::new())),
        default_registry().with_project_root(root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-embed".to_string(),
    )
    .with_symbol_store(db_path)
    .with_embedding_provider(provider);
    (rt, root_str)
}

// Tests

#[test]
fn index_embed_no_store_emits_not_available() {
    let mut rt = make_runtime(Vec::<String>::new());
    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let msg = events.iter().find_map(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            Some(m.clone())
        } else {
            None
        }
    });
    assert!(
        msg.as_deref().unwrap_or("").contains("not available"),
        "expected 'not available' message, got: {msg:?}"
    );
}

#[test]
fn index_embed_no_provider_emits_unconfigured_message() {
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    seed_symbols(&store, ".", &["fn_a"]);
    drop(store);

    use crate::core::config::Config;
    use crate::runtime::ProjectRoot;
    use crate::tools::default_registry;
    use std::path::PathBuf;

    let root = ProjectRoot::new(PathBuf::from(".")).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        root.clone(),
        Box::new(TestBackend::new(Vec::<String>::new())),
        default_registry().with_project_root(root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-embed-no-provider".to_string(),
    )
    .with_symbol_store(db.path());

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let msg = events.iter().find_map(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            Some(m.clone())
        } else {
            None
        }
    });
    assert!(
        msg.as_deref().unwrap_or("").contains("no embedding model"),
        "expected unconfigured message, got: {msg:?}"
    );
}

#[test]
fn index_embed_no_symbols_emits_build_first_message() {
    let db = NamedTempFile::new().unwrap();
    let _store = open_store(db.path());
    drop(_store);

    let provider = Arc::new(ConstantEmbedProvider(vec![1.0, 0.0]));
    let (mut rt, _root) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let msg = events.iter().find_map(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            Some(m.clone())
        } else {
            None
        }
    });
    assert!(
        msg.as_deref().unwrap_or("").contains("no symbols indexed"),
        "expected 'no symbols indexed', got: {msg:?}"
    );
}

#[test]
fn index_embed_stores_embeddings_for_symbols() {
    let root = canonical_root();
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    seed_symbols(&store, &root, &["alpha", "beta"]);
    drop(store);

    let provider = Arc::new(ConstantEmbedProvider(vec![0.5, 0.5]));
    let (mut rt, root) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let stored = events.iter().any(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            m.contains("embeddings stored")
        } else {
            false
        }
    });
    assert!(stored, "expected 'embeddings stored' in events: {events:?}");

    let store2 = open_store(db.path());
    assert_eq!(store2.embedding_count(&root).unwrap(), 2);
}

#[test]
fn index_embed_provider_failure_emits_failed_message() {
    let root = canonical_root();
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    seed_symbols(&store, &root, &["fn_x"]);
    drop(store);

    let provider = Arc::new(FailingEmbedProvider);
    let (mut rt, _) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let has_failed = events.iter().any(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            m.contains("failed")
        } else {
            false
        }
    });
    assert!(has_failed, "expected failure message in events");
}

#[test]
fn index_embed_clears_on_model_change() {
    let root = canonical_root();
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    seed_symbols(&store, &root, &["sym_a"]);
    let ids = store.all_symbols_ranked(&root).unwrap();
    store
        .upsert_embeddings(
            &root,
            &[(ids[0].0, vec![1.0, 0.0], "old-model".to_string())],
        )
        .unwrap();
    assert_eq!(
        store.get_embedding_model(&root).unwrap().as_deref(),
        Some("old-model")
    );
    drop(store);

    let provider = Arc::new(ConstantEmbedProvider(vec![0.1, 0.9]));
    let (mut rt, root) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let has_clearing = events.iter().any(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            m.contains("model changed")
        } else {
            false
        }
    });
    assert!(
        has_clearing,
        "expected model-change clearing message; events: {events:?}"
    );

    let store2 = open_store(db.path());
    assert_ne!(
        store2.get_embedding_model(&root).unwrap().as_deref(),
        Some("old-model")
    );
}

#[test]
fn index_embed_emits_per_chunk_progress_messages() {
    let root = canonical_root();
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    // Seed 40 symbols — chunk_size is 32, so this produces 2 chunk iterations.
    let names: Vec<String> = (0..40).map(|i| format!("fn_{i}")).collect();
    let name_strs: Vec<&str> = names.iter().map(String::as_str).collect();
    seed_symbols(&store, &root, &name_strs);
    drop(store);

    let provider = Arc::new(ConstantEmbedProvider(vec![0.1, 0.9]));
    let (mut rt, _) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let chunk_msgs: Vec<&str> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::SystemMessage(m) = e {
                if m.contains("embed: chunk ") {
                    Some(m.as_str())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    assert!(
        chunk_msgs.len() >= 2,
        "expected at least 2 per-chunk progress messages; got {}: {events:?}",
        chunk_msgs.len()
    );
}

#[test]
fn index_embed_caps_at_2000_symbols_with_warning() {
    let root = canonical_root();
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    let names: Vec<String> = (0..2001).map(|i| format!("fn_{i}")).collect();
    let name_strs: Vec<&str> = names.iter().map(String::as_str).collect();
    seed_symbols(&store, &root, &name_strs);
    drop(store);

    let provider = Arc::new(ConstantEmbedProvider(vec![0.0, 1.0]));
    let (mut rt, root) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);
    let has_cap_warning = events.iter().any(|e| {
        if let RuntimeEvent::SystemMessage(m) = e {
            m.contains("capping at 2000")
        } else {
            false
        }
    });
    assert!(
        has_cap_warning,
        "expected 2000 cap warning; events: {events:?}"
    );

    let store2 = open_store(db.path());
    assert_eq!(store2.embedding_count(&root).unwrap(), 2000);
}

#[test]
fn index_embed_chunk_failure_skips_and_continues_pipeline() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FirstChunkFailProvider {
        calls: AtomicUsize,
    }
    impl EmbeddingProvider for FirstChunkFailProvider {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(AppError::Runtime("transient network error".to_string()))
            } else {
                Ok(texts.iter().map(|_| vec![0.5f32, 0.5]).collect())
            }
        }
    }

    let root = canonical_root();
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    // 40 symbols = 2 chunks (32 + 8); first chunk fails, second succeeds.
    let names: Vec<String> = (0..40).map(|i| format!("fn_{i}")).collect();
    let name_strs: Vec<&str> = names.iter().map(String::as_str).collect();
    seed_symbols(&store, &root, &name_strs);
    drop(store);

    let provider = Arc::new(FirstChunkFailProvider {
        calls: AtomicUsize::new(0),
    });
    let (mut rt, root) = make_runtime_with_store_and_provider(db.path(), provider);

    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbed);

    let has_skip = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("skipping")));
    assert!(
        has_skip,
        "failed chunk must emit 'skipping' message; events: {events:?}"
    );

    let has_stored = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("embeddings stored")));
    assert!(
        has_stored,
        "pipeline must complete after chunk failure; events: {events:?}"
    );

    // Only the second chunk's 8 symbols should be stored.
    let store2 = open_store(db.path());
    let count = store2.embedding_count(&root).unwrap();
    assert_eq!(
        count, 8,
        "only second chunk (8 symbols) must be persisted; got {count}"
    );
}

#[test]
fn index_embed_chunk_after_reset_is_silent_no_op() {
    // Verifies the stale-dispatch guard: IndexEmbedChunk with no pending_embed
    // (either because embed was never started or because Reset cleared it) must
    // return silently without emitting any events or panicking.
    let db = NamedTempFile::new().unwrap();
    let store = open_store(db.path());
    let root = canonical_root();
    seed_symbols(&store, &root, &["fn_a", "fn_b", "fn_c"]);
    drop(store);

    let provider = Arc::new(ConstantEmbedProvider(vec![0.1, 0.2]));
    let (mut rt, _) = make_runtime_with_store_and_provider(db.path(), provider);

    // Complete a full embed (synchronous recursive dispatch clears pending_embed).
    let _ = collect_events(&mut rt, RuntimeRequest::IndexEmbed);

    // Reset the session — must clear pending_embed.
    let _ = collect_events(&mut rt, RuntimeRequest::Reset);

    // A stale IndexEmbedChunk must be a silent no-op.
    let events = collect_events(&mut rt, RuntimeRequest::IndexEmbedChunk);
    assert!(
        events.is_empty(),
        "expected no events from stale IndexEmbedChunk after reset; got: {events:?}"
    );
}
