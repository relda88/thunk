use std::fs;

use super::*;

fn git_config_user(root: &std::path::Path) {
    for args in [
        vec!["config", "user.email", "thunk@example.invalid"],
        vec!["config", "user.name", "thunk"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
    }
}

fn git_add(root: &std::path::Path, path: &str) {
    std::process::Command::new("git")
        .args(["add", path])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
}

fn git_init_with_commit(root: &std::path::Path) {
    init_git_repo(root);
    git_config_user(root);
    // Create an initial commit so HEAD exists.
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

#[test]
fn commit_nothing_staged_emits_system_message() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    git_init_with_commit(tmp.path());
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    let events = collect_events(&mut rt, RuntimeRequest::Commit { message: None });

    let has_nothing_to_commit = events.iter().any(|e| match e {
        RuntimeEvent::SystemMessage(msg) => msg.contains("nothing to commit"),
        _ => false,
    });
    assert!(
        has_nothing_to_commit,
        "commit with nothing staged must emit 'nothing to commit': {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ApprovalRequired { .. })),
        "commit with nothing staged must not surface approval: {events:?}"
    );
    assert!(!has_failed(&events), "must not emit Failed: {events:?}");
}

#[test]
fn commit_with_explicit_message_surfaces_approval() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    git_init_with_commit(tmp.path());
    fs::write(tmp.path().join("hello.txt"), "hello\n").unwrap();
    git_add(tmp.path(), "hello.txt");

    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Commit {
            message: Some("feat: add hello.txt".to_string()),
        },
    );

    let approval = events.iter().find_map(|e| {
        if let RuntimeEvent::ApprovalRequired { pending, .. } = e {
            Some(pending.clone())
        } else {
            None
        }
    });
    assert!(
        approval.is_some(),
        "commit with staged file must surface ApprovalRequired: {events:?}"
    );
    let pending = approval.unwrap();
    assert_eq!(pending.tool_name, "git_commit");
    assert_eq!(pending.payload, "feat: add hello.txt");
    assert!(
        pending.summary.contains("feat: add hello.txt"),
        "summary must include commit subject: {}",
        pending.summary
    );
    assert_eq!(pending.risk, RiskLevel::High);
    assert!(!has_failed(&events), "must not emit Failed: {events:?}");
}

#[test]
fn commit_with_explicit_message_executes_on_approve() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    git_init_with_commit(tmp.path());
    fs::write(tmp.path().join("world.txt"), "world\n").unwrap();
    git_add(tmp.path(), "world.txt");
    git_config_user(tmp.path());

    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    // Submit the commit request — produces ApprovalRequired.
    collect_events(
        &mut rt,
        RuntimeRequest::Commit {
            message: Some("feat: add world.txt".to_string()),
        },
    );

    // Approve — this should execute git_commit and emit a final answer.
    let approve_events = collect_events(&mut rt, RuntimeRequest::Approve);

    assert!(
        !has_failed(&approve_events),
        "approve must not emit Failed: {approve_events:?}"
    );
    let has_answer = approve_events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::AnswerReady(_)));
    assert!(
        has_answer,
        "approve must emit AnswerReady after commit: {approve_events:?}"
    );
}

#[test]
fn commit_blocked_while_approval_pending() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    git_init_with_commit(tmp.path());
    fs::write(tmp.path().join("a.txt"), "a\n").unwrap();
    git_add(tmp.path(), "a.txt");

    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    // First commit surfaces approval.
    collect_events(
        &mut rt,
        RuntimeRequest::Commit {
            message: Some("feat: first".to_string()),
        },
    );

    // Second commit while approval is pending must fail.
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Commit {
            message: Some("feat: second".to_string()),
        },
    );
    assert!(
        has_failed(&events),
        "commit while approval pending must emit Failed: {events:?}"
    );
}
