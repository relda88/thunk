use tempfile::NamedTempFile;

use crate::runtime::memory::MemoryManager;
use crate::storage::memory::{MemorySource, MemoryStore};

use super::*;

fn attach_memory_manager(runtime: &mut Runtime, path: &std::path::Path) {
    let store = MemoryStore::open(path).unwrap();
    let manager = MemoryManager::new(store, None);
    runtime.memory_manager = Some(manager);
}

fn seed_stale_fact(runtime: &mut Runtime, text: &str) {
    // A never-recalled fact (NULL last_recalled_at) is stale against any cutoff.
    // Global scope (None) is admitted regardless of the current project scope.
    let mgr = runtime.memory_manager.as_ref().unwrap();
    mgr.store
        .upsert_fact(
            text,
            "preference",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();
}

#[test]
fn proactive_scan_emits_proposal_for_stale_fact() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());
    seed_stale_fact(&mut runtime, "I prefer concise commit messages");

    let mut events = Vec::new();
    runtime.proactive_scan(&mut |e| events.push(e));

    let proposal = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }));
    assert!(
        proposal.is_some(),
        "expected MemoryProposalRequired for a stale fact"
    );
    if let Some(RuntimeEvent::MemoryProposalRequired { fact, source, .. }) = proposal {
        assert_eq!(fact, "I prefer concise commit messages");
        assert_eq!(source, "reflection");
    }
}

#[test]
fn proactive_scan_suppressed_by_dnd() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());
    seed_stale_fact(&mut runtime, "I prefer concise commit messages");

    // Enable do-not-disturb via the toggle, then scan.
    collect_events(
        &mut runtime,
        RuntimeRequest::DndToggle {
            enabled: Some(true),
        },
    );

    let mut events = Vec::new();
    runtime.proactive_scan(&mut |e| events.push(e));

    assert!(
        events.is_empty(),
        "DND must suppress all proactive events, got: {events:?}"
    );
}

#[test]
fn proactive_scan_respects_min_interval() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());
    seed_stale_fact(&mut runtime, "I prefer concise commit messages");

    // First scan surfaces the stale fact and records the scan time.
    let mut first = Vec::new();
    runtime.proactive_scan(&mut |e| first.push(e));
    assert!(
        first
            .iter()
            .any(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. })),
        "first scan should surface the stale fact"
    );

    // Second scan in quick succession is below the minimum interval floor — silent.
    let mut second = Vec::new();
    runtime.proactive_scan(&mut |e| second.push(e));
    assert!(
        second.is_empty(),
        "second scan within the minimum interval must emit nothing, got: {second:?}"
    );
}
