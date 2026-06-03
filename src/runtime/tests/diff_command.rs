use std::fs;

use crate::runtime::types::DiffMode;

use super::*;

fn git_commit_file(root: &std::path::Path, file: &str, contents: &str) {
    fs::write(root.join(file), contents).unwrap();
    std::process::Command::new("git")
        .args(["add", file])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "-c",
            "user.email=thunk@example.invalid",
            "-c",
            "user.name=thunk",
            "commit",
            "-m",
            "initial",
        ])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
}

fn has_info_message(events: &[RuntimeEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::InfoMessage(_)))
}

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
fn diff_working_tree_emits_info_message() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    git_commit_file(tmp.path(), "foo.txt", "old\n");
    // Make an unstaged change.
    fs::write(tmp.path().join("foo.txt"), "new\n").unwrap();

    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Diff {
            mode: DiffMode::WorkingTree,
        },
    );

    assert!(
        has_info_message(&events),
        "diff working tree must emit InfoMessage: {events:?}"
    );
    assert!(!has_failed(&events), "must not emit Failed: {events:?}");
}

#[test]
fn diff_last_emits_changes_since_session_start() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    git_commit_file(tmp.path(), "foo.txt", "old\n");
    // Runtime captures session_start_ref at construction time (commit A's hash).
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());
    // Make an unstaged change after runtime is constructed.
    fs::write(tmp.path().join("foo.txt"), "new\n").unwrap();

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Diff {
            mode: DiffMode::SessionStart,
        },
    );

    assert!(
        has_info_message(&events),
        "diff last must emit InfoMessage: {events:?}"
    );
    assert!(!has_failed(&events), "must not emit Failed: {events:?}");
}

#[test]
fn diff_last_no_session_ref_emits_error() {
    use tempfile::TempDir;

    // TempDir with no commits — capture_session_head returns None.
    let tmp = TempDir::new().unwrap();
    init_git_repo(tmp.path());
    // No commits: session_start_ref will be None.
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Diff {
            mode: DiffMode::SessionStart,
        },
    );

    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("no session baseline")),
        "must emit 'no session baseline' SystemMessage: {msgs:?}"
    );
    assert!(
        !has_info_message(&events),
        "must not emit InfoMessage when no session ref: {events:?}"
    );
    assert!(!has_failed(&events), "must not emit Failed: {events:?}");
}
