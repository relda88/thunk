use crate::runtime::types::Activity;

use super::*;

fn git_commit(root: &std::path::Path) {
    std::process::Command::new("git")
        .args([
            "-c",
            "user.email=thunk@example.invalid",
            "-c",
            "user.name=thunk",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
}

fn git_create_branch(root: &std::path::Path, name: &str) {
    std::process::Command::new("git")
        .args(["branch", name])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
}

#[test]
fn branch_create_surfaces_approval() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    git_commit(tmp.path());
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::BranchCreate {
            name: "feat/test".to_string(),
            start_point: None,
        },
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. })),
        "branch create must surface an approval: {events:?}"
    );
    assert!(
        !has_failed(&events),
        "branch create must not fail before approval: {events:?}"
    );

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);

    assert!(
        !has_failed(&approve_events),
        "branch create approval must succeed: {approve_events:?}"
    );
    assert!(
        approve_events.iter().any(
            |e| matches!(e, RuntimeEvent::ToolCallFinished { name, summary: Some(_) } if name == "git_branch_create")
        ),
        "approved create must emit ToolCallFinished: {approve_events:?}"
    );
}

#[test]
fn branch_switch_surfaces_approval() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    git_commit(tmp.path());
    git_create_branch(tmp.path(), "feat/switch-target");
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::BranchSwitch {
            name: "feat/switch-target".to_string(),
        },
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. })),
        "branch switch must surface an approval: {events:?}"
    );
    assert!(
        !has_failed(&events),
        "branch switch must not fail before approval: {events:?}"
    );

    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);

    assert!(
        !has_failed(&approve_events),
        "branch switch approval must succeed: {approve_events:?}"
    );
    assert!(
        approve_events.iter().any(
            |e| matches!(e, RuntimeEvent::ToolCallFinished { name, summary: Some(_) } if name == "git_branch_switch")
        ),
        "approved switch must emit ToolCallFinished: {approve_events:?}"
    );
}

#[test]
fn branch_switch_triggers_session_reset() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    git_commit(tmp.path());
    git_create_branch(tmp.path(), "feat/reset-target");
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    collect_events(
        &mut rt,
        RuntimeRequest::BranchSwitch {
            name: "feat/reset-target".to_string(),
        },
    );
    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);

    assert!(
        !has_failed(&approve_events),
        "branch switch must succeed: {approve_events:?}"
    );
    // handle_reset() emits ActivityChanged(Idle) — confirm session reset fired.
    assert!(
        approve_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ActivityChanged(Activity::Idle))),
        "branch switch approval must emit ActivityChanged(Idle) from session reset: {approve_events:?}"
    );
    // After reset, conversation contains only the system prompt (1 message).
    assert_eq!(
        rt.messages_snapshot().len(),
        1,
        "conversation must be reset to system-prompt-only after branch switch"
    );
}

#[test]
fn branch_list_returns_info_message() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    let events = collect_events(&mut rt, RuntimeRequest::GitBranch);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::InfoMessage(_))),
        "GitBranch must emit an InfoMessage: {events:?}"
    );
    assert!(!has_failed(&events), "GitBranch must not fail: {events:?}");
}
