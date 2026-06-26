use tempfile::NamedTempFile;

use crate::runtime::memory::MemoryManager;
use crate::runtime::types::{RuntimeEvent, RuntimeRequest};
use crate::storage::memory::MemoryStore;

use super::*;

fn attach_memory_manager(runtime: &mut Runtime, path: &std::path::Path) {
    let store = MemoryStore::open(path).unwrap();
    let manager = MemoryManager::new(store, None);
    runtime.memory_manager = Some(manager);
}

/// Seed the conversation with one user+assistant exchange so human_visible_snapshot
/// returns >= 2 messages. Consumes the first backend response slot.
fn seed_conversation(runtime: &mut Runtime) {
    collect_events(
        runtime,
        RuntimeRequest::Submit {
            text: "I prefer tabs over spaces for all my projects".to_string(),
        },
    );
}

#[test]
fn reflect_valid_tags_emits_proposals() {
    let tmp = NamedTempFile::new().unwrap();
    // slot 0: response to seed Submit; slot 1: reflection extraction output
    let reflection_output =
        "[REMEMBER: prefers tabs over spaces | preference]\n[REMEMBER: uses Rust for backend | workflow]";
    let mut runtime = make_runtime(vec!["Got it.", reflection_output]);
    attach_memory_manager(&mut runtime, tmp.path());

    seed_conversation(&mut runtime);

    let events = collect_events(&mut runtime, RuntimeRequest::Reflect);

    let proposals: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }))
        .collect();
    assert!(
        !proposals.is_empty(),
        "expected at least one MemoryProposalRequired"
    );

    if let Some(RuntimeEvent::MemoryProposalRequired {
        fact,
        category,
        delete,
        ..
    }) = proposals.first()
    {
        assert_eq!(fact, "prefers tabs over spaces");
        assert_eq!(category, "preference");
        assert!(!delete, "reflect proposals must not be deletions");
    }
}

#[test]
fn reflect_no_tags_emits_nothing_to_remember() {
    let tmp = NamedTempFile::new().unwrap();
    // slot 0: seed response; slot 1: model outputs prose with no tags
    let prose_output = "I didn't find any particularly memorable facts from this conversation.";
    let mut runtime = make_runtime(vec!["Got it.", prose_output]);
    attach_memory_manager(&mut runtime, tmp.path());

    seed_conversation(&mut runtime);

    let events = collect_events(&mut runtime, RuntimeRequest::Reflect);

    let has_proposal = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }));
    assert!(
        !has_proposal,
        "no proposal expected when model emits no tags"
    );

    let sys_msg = events.iter().find_map(|e| {
        if let RuntimeEvent::SystemMessage(msg) = e {
            Some(msg.as_str())
        } else {
            None
        }
    });
    assert_eq!(
        sys_msg,
        Some("reflect: nothing to remember"),
        "expected 'nothing to remember' system message"
    );
}

#[test]
fn reflect_mixed_output_only_valid_tags_proposed() {
    let tmp = NamedTempFile::new().unwrap();
    // slot 0: seed response; slot 1: mix of valid tags and garbage lines
    let mixed_output = "Here is what I found:\n\
                        [REMEMBER: prefers dark mode | preference]\n\
                        This line is just explanation text and should be ignored.\n\
                        not a tag either\n\
                        [REMEMBER: uses Neovim daily | workflow]";
    let mut runtime = make_runtime(vec!["Got it.", mixed_output]);
    attach_memory_manager(&mut runtime, tmp.path());

    seed_conversation(&mut runtime);

    let events = collect_events(&mut runtime, RuntimeRequest::Reflect);

    let proposals: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }))
        .collect();
    // Only the first proposal is surfaced immediately; the second is queued
    assert_eq!(proposals.len(), 1, "first proposal emitted, second queued");

    if let Some(RuntimeEvent::MemoryProposalRequired { fact, .. }) = proposals.first() {
        assert_eq!(fact, "prefers dark mode");
    }

    // Verify the queue has the second candidate by approving and checking for the next proposal
    let approve_events = collect_events(&mut runtime, RuntimeRequest::MemoryApprove);
    let next_proposal = approve_events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::MemoryProposalRequired { .. }));
    assert!(
        next_proposal.is_some(),
        "second proposal should surface after approving first"
    );

    if let Some(RuntimeEvent::MemoryProposalRequired { fact, .. }) = next_proposal {
        assert_eq!(fact, "uses Neovim daily");
    }
}
