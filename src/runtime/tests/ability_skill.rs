use tempfile::TempDir;

use super::*;

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

#[test]
fn ability_toggle_sets_active() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("debug".into()),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("ability: debug")),
        "expected 'ability: debug' in system messages, got: {msgs:?}"
    );
}

#[test]
fn ability_toggle_off_clears() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("debug".into()),
        },
    );
    let events = collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("off".into()),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("ability: cleared")),
        "expected 'ability: cleared' in system messages, got: {msgs:?}"
    );
}

#[test]
fn ability_toggle_status_when_none() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(&mut rt, RuntimeRequest::AbilityToggle { name: None });
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("ability: none")),
        "expected 'ability: none' in system messages, got: {msgs:?}"
    );
}

#[test]
fn ability_toggle_invalid_name_emits_error() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("nonexistent_ability_xyz".into()),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("ability error")),
        "expected 'ability error' in system messages, got: {msgs:?}"
    );
}

#[test]
fn skill_toggle_sets_active() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::SkillToggle {
            name: Some("concise".into()),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("skill: concise")),
        "expected 'skill: concise' in system messages, got: {msgs:?}"
    );
}

#[test]
fn ability_and_skill_both_active() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());

    let ability_events = collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("review".into()),
        },
    );
    let ability_msgs = system_messages(&ability_events);
    assert!(
        ability_msgs.iter().any(|m| m.contains("ability: review")),
        "expected 'ability: review' in system messages, got: {ability_msgs:?}"
    );

    let skill_events = collect_events(
        &mut rt,
        RuntimeRequest::SkillToggle {
            name: Some("thorough".into()),
        },
    );
    let skill_msgs = system_messages(&skill_events);
    assert!(
        skill_msgs.iter().any(|m| m.contains("skill: thorough")),
        "expected 'skill: thorough' in system messages, got: {skill_msgs:?}"
    );
}
