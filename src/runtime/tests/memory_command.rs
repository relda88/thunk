use tempfile::NamedTempFile;

use crate::runtime::memory::MemoryManager;
use crate::storage::memory::{MemorySource, MemoryStore};

use super::*;

fn attach_memory_manager(runtime: &mut Runtime, path: &std::path::Path) {
    let store = MemoryStore::open(path).unwrap();
    let manager = MemoryManager::new(store, None);
    runtime.memory_manager = Some(manager);
}

#[test]
fn remember_command_emits_proposal() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::Remember {
            fact: "I prefer tabs over spaces".to_string(),
        },
    );

    let proposal = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }));
    assert!(proposal.is_some(), "expected MemoryProposalRequired event");

    if let Some(RuntimeEvent::MemoryProposalRequired { fact, category, .. }) = proposal {
        assert_eq!(fact, "I prefer tabs over spaces");
        assert_eq!(category, "user");
    }

    // Not yet persisted
    let mgr = runtime.memory_manager.as_ref().unwrap();
    let stored = mgr.store.list_facts(None).unwrap();
    assert!(
        stored.is_empty(),
        "fact must not be persisted before approval"
    );
}

#[test]
fn memory_approve_persists_fact() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    // Propose via /remember
    collect_events(
        &mut runtime,
        RuntimeRequest::Remember {
            fact: "I use Rust for all backend work".to_string(),
        },
    );

    // Approve
    let approve_events = collect_events(&mut runtime, RuntimeRequest::MemoryApprove);

    let cleared = approve_events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::MemoryProposalCleared));
    assert!(cleared, "expected MemoryProposalCleared after approve");

    // Now persisted
    let mgr = runtime.memory_manager.as_ref().unwrap();
    let stored = mgr.store.list_facts(None).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].text, "I use Rust for all backend work");
}

#[test]
fn memory_reject_does_not_persist() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    // Propose via /remember
    collect_events(
        &mut runtime,
        RuntimeRequest::Remember {
            fact: "ephemeral fact".to_string(),
        },
    );

    // Reject
    let reject_events = collect_events(&mut runtime, RuntimeRequest::MemoryReject);

    let cleared = reject_events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::MemoryProposalCleared));
    assert!(cleared, "expected MemoryProposalCleared after reject");

    // Not persisted
    let mgr = runtime.memory_manager.as_ref().unwrap();
    let stored = mgr.store.list_facts(None).unwrap();
    assert!(
        stored.is_empty(),
        "fact must not be persisted after rejection"
    );
}

#[test]
fn intent_detection_triggers_proposal() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::Submit {
            text: "remember I use Rust for all backend work".to_string(),
        },
    );

    let proposal = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }));
    assert!(
        proposal.is_some(),
        "expected MemoryProposalRequired from intent detection"
    );

    if let Some(RuntimeEvent::MemoryProposalRequired { fact, .. }) = proposal {
        assert_eq!(fact, "I use Rust for all backend work");
    }
}

#[test]
fn memory_list_returns_facts() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    let mgr = runtime.memory_manager.as_ref().unwrap();
    mgr.store
        .upsert_fact(
            "I prefer tabs",
            "user",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();
    mgr.store
        .upsert_fact(
            "I use Rust for backends",
            "user",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

    let events = collect_events(&mut runtime, RuntimeRequest::MemoryList);

    let info = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::InfoMessage(_)));
    assert!(info.is_some(), "expected InfoMessage from /memory");
    if let Some(RuntimeEvent::InfoMessage(text)) = info {
        assert!(text.contains("I prefer tabs"), "list missing first fact");
        assert!(
            text.contains("I use Rust for backends"),
            "list missing second fact"
        );
    }
}

#[test]
fn forget_command_emits_deletion_proposal() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    let mgr = runtime.memory_manager.as_ref().unwrap();
    let id = mgr
        .store
        .upsert_fact(
            "I prefer dark mode",
            "user",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

    let events = collect_events(&mut runtime, RuntimeRequest::MemoryForget { id });

    let proposal = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }));
    assert!(
        proposal.is_some(),
        "expected MemoryProposalRequired from /forget"
    );

    if let Some(RuntimeEvent::MemoryProposalRequired { fact, delete, .. }) = proposal {
        assert_eq!(fact, "I prefer dark mode");
        assert!(delete, "expected delete: true for forget proposal");
    }
}

#[test]
fn memory_delete_approve_removes_fact() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    let mgr = runtime.memory_manager.as_ref().unwrap();
    let id = mgr
        .store
        .upsert_fact(
            "temporary preference",
            "user",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

    collect_events(&mut runtime, RuntimeRequest::MemoryForget { id });
    let approve_events = collect_events(&mut runtime, RuntimeRequest::MemoryApprove);

    let cleared = approve_events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::MemoryProposalCleared));
    assert!(
        cleared,
        "expected MemoryProposalCleared after approve-delete"
    );

    let mgr = runtime.memory_manager.as_ref().unwrap();
    let stored = mgr.store.list_facts(None).unwrap();
    assert!(stored.is_empty(), "fact must be deleted after approve");
}

#[test]
fn memory_delete_reject_preserves_fact() {
    let tmp = NamedTempFile::new().unwrap();
    let mut runtime = make_runtime(Vec::<String>::new());
    attach_memory_manager(&mut runtime, tmp.path());

    let mgr = runtime.memory_manager.as_ref().unwrap();
    let id = mgr
        .store
        .upsert_fact(
            "keep this fact",
            "user",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

    collect_events(&mut runtime, RuntimeRequest::MemoryForget { id });
    let reject_events = collect_events(&mut runtime, RuntimeRequest::MemoryReject);

    let cleared = reject_events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::MemoryProposalCleared));
    assert!(
        cleared,
        "expected MemoryProposalCleared after reject-delete"
    );

    let mgr = runtime.memory_manager.as_ref().unwrap();
    let stored = mgr.store.list_facts(None).unwrap();
    assert_eq!(stored.len(), 1, "fact must be preserved after reject");
    assert_eq!(stored[0].text, "keep this fact");
}
