use crate::llm::backend::Role;

use super::*;

#[test]
fn periodic_refresh_message_injected_when_enabled() {
    let (rt, requests) = make_runtime_with_recorded_requests(vec!["Done."]);
    let mut rt = rt.with_prompt_physics_enabled();
    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "what does main do".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");
    assert!(
        first
            .messages
            .iter()
            .any(|m| { m.role == Role::System && m.content.contains("runtime owns control flow") }),
        "periodic refresh message must appear in backend request when enabled: {:?}",
        first.messages
    );
}

#[test]
fn periodic_refresh_message_absent_when_disabled() {
    let (mut rt, requests) = make_runtime_with_recorded_requests(vec!["Done."]);
    // Default is now enabled=true; explicitly disable for this test via the toggle.
    collect_events(
        &mut rt,
        RuntimeRequest::PromptPhysicsToggle {
            enabled: Some(false),
        },
    );
    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "what does main do".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");
    assert!(
        !first
            .messages
            .iter()
            .any(|m| { m.role == Role::System && m.content.contains("runtime owns control flow") }),
        "periodic refresh message must not appear when disabled: {:?}",
        first.messages
    );
}

#[test]
fn periodic_refresh_message_appears_after_snapshot_hint() {
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    use crate::core::config::Config;
    use crate::tools::default_registry;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(RecordingBackend::new(vec!["Done."], Arc::clone(&requests))),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
    )
    .with_prompt_physics_enabled();

    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "where is main defined".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");

    let snapshot_pos = first
        .messages
        .iter()
        .position(|m| m.role == Role::System && m.content.starts_with("[project snapshot]"));
    let refresh_pos = first
        .messages
        .iter()
        .position(|m| m.role == Role::System && m.content.contains("runtime owns control flow"));

    assert!(
        refresh_pos.is_some(),
        "periodic refresh message must be present: {:?}",
        first.messages
    );
    if let (Some(snap), Some(refresh)) = (snapshot_pos, refresh_pos) {
        assert!(
            refresh > snap,
            "periodic refresh must appear after snapshot hint (snap={snap}, refresh={refresh})"
        );
    }
}

#[test]
fn recency_field_appears_after_periodic_refresh() {
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    use crate::core::config::Config;
    use crate::tools::default_registry;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let mut rt = Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(RecordingBackend::new(vec!["Done."], Arc::clone(&requests))),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
    )
    .with_prompt_physics_enabled();

    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "where is main defined".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");

    let refresh_pos = first
        .messages
        .iter()
        .position(|m| m.role == Role::System && m.content.contains("runtime owns control flow"));
    let recency_pos = first
        .messages
        .iter()
        .position(|m| m.role == Role::System && m.content.contains("[thunk: current context]"));

    assert!(
        recency_pos.is_some(),
        "recency field must be present when physics enabled: {:?}",
        first.messages
    );
    if let (Some(refresh), Some(recency)) = (refresh_pos, recency_pos) {
        assert!(
            recency > refresh,
            "recency field must appear after periodic refresh (refresh={refresh}, recency={recency})"
        );
    }
}

#[test]
fn ability_content_appears_in_generation_request() {
    let (mut rt, requests) = make_runtime_with_recorded_requests(vec!["Done."]);

    // Activate the debug ability — this syncs into prompt_physics.active_ability.
    collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("debug".into()),
        },
    );

    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "what does main do".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");

    let ability_pos = first
        .messages
        .iter()
        .position(|m| m.role == Role::System && m.content.contains("[ability: debug]"));
    let surface_pos = first
        .messages
        .iter()
        .position(|m| m.role == Role::System && m.content.contains("Active tool surface:"));

    assert!(
        ability_pos.is_some(),
        "ability content must appear in backend request when ability is active: {:?}",
        first.messages
    );
    if let (Some(ability), Some(surface)) = (ability_pos, surface_pos) {
        assert!(
            ability < surface,
            "ability primacy block must appear before surface hint (ability={ability}, surface={surface})"
        );
    }
}

#[test]
fn skill_content_appears_in_periodic_refresh() {
    let (mut rt, requests) = make_runtime_with_recorded_requests(vec!["Done."]);

    // Activate the concise skill — syncs into prompt_physics.active_skill.
    collect_events(
        &mut rt,
        RuntimeRequest::SkillToggle {
            name: Some("concise".into()),
        },
    );

    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "what does main do".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");

    assert!(
        first
            .messages
            .iter()
            .any(|m| m.role == Role::System && m.content.contains("Style:")),
        "skill style instructions must appear in periodic refresh when skill is active: {:?}",
        first.messages
    );
    assert!(
        !first
            .messages
            .iter()
            .any(|m| m.role == Role::System && m.content.contains("[ability:")),
        "periodic refresh must not contain ability block when only skill is active: {:?}",
        first.messages
    );
}
