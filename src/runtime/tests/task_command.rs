use rusqlite::Connection;

use crate::storage::session::schema;
use crate::storage::tasks::{PlanStatus, TaskStatus, TaskStore};

use super::*;

fn attach_task_store(runtime: &mut Runtime, path: &std::path::Path) {
    let init_conn = Connection::open(path).unwrap();
    schema::initialize(&init_conn).unwrap();
    drop(init_conn);
    let store = TaskStore::open(path).unwrap();
    runtime.task_store = Some(store);
}

/// Creates an active plan with one task (step 1) and returns (plan_id, task_id).
fn setup_active_plan(runtime: &mut Runtime, project_root: &str) -> (String, String) {
    let store = runtime.task_store.as_ref().unwrap();
    let plan_id = store
        .create_plan("test-session", project_root, "test goal")
        .unwrap();
    let task_id = store
        .add_task(
            &plan_id,
            "test-session",
            project_root,
            1,
            "First step",
            "Do the thing",
        )
        .unwrap();
    store
        .update_plan_status(&plan_id, PlanStatus::Active)
        .unwrap();
    (plan_id, task_id)
}

#[test]
fn task_execute_marks_in_progress_and_fires_answer_ready() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["I've executed the step."], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let (plan_id, _) = setup_active_plan(&mut runtime, &project_root);

    let events = collect_events(&mut runtime, RuntimeRequest::TaskExecute { step: 1 });

    let tasks = runtime
        .task_store
        .as_ref()
        .unwrap()
        .list_tasks(&plan_id)
        .unwrap();
    assert_eq!(
        tasks[0].status,
        TaskStatus::InProgress,
        "task must be marked InProgress before run_turns"
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::AnswerReady(_))),
        "expected AnswerReady after task execution, got: {events:?}"
    );
}

#[test]
fn task_execute_no_active_plan_emits_error() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let events = collect_events(&mut runtime, RuntimeRequest::TaskExecute { step: 1 });

    let has_error = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("no active plan")));
    assert!(
        has_error,
        "expected 'no active plan' message, got: {events:?}"
    );
}

#[test]
fn task_execute_invalid_step_emits_error() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    setup_active_plan(&mut runtime, &project_root);

    let events = collect_events(&mut runtime, RuntimeRequest::TaskExecute { step: 99 });

    let has_error = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("not found")));
    assert!(has_error, "expected 'not found' message, got: {events:?}");
}

#[test]
fn task_complete_updates_status() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let (plan_id, _) = setup_active_plan(&mut runtime, &project_root);

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::TaskComplete {
            step: 1,
            summary: None,
        },
    );

    let tasks = runtime
        .task_store
        .as_ref()
        .unwrap()
        .list_tasks(&plan_id)
        .unwrap();
    assert_eq!(tasks[0].status, TaskStatus::Completed);

    let has_msg = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("completed")));
    assert!(has_msg, "expected 'completed' message, got: {events:?}");
}

#[test]
fn task_complete_with_summary_persists() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let (plan_id, _) = setup_active_plan(&mut runtime, &project_root);

    collect_events(
        &mut runtime,
        RuntimeRequest::TaskComplete {
            step: 1,
            summary: Some("done and done".to_string()),
        },
    );

    let tasks = runtime
        .task_store
        .as_ref()
        .unwrap()
        .list_tasks(&plan_id)
        .unwrap();
    assert_eq!(tasks[0].status, TaskStatus::Completed);
    assert_eq!(tasks[0].result_summary, "done and done");
}

#[test]
fn task_complete_all_done_marks_plan_completed() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    // One-task plan: completing it must mark the plan completed.
    setup_active_plan(&mut runtime, &project_root);

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::TaskComplete {
            step: 1,
            summary: None,
        },
    );

    let has_plan_msg = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("all steps completed")));
    assert!(
        has_plan_msg,
        "expected 'all steps completed' message, got: {events:?}"
    );

    let active = runtime
        .task_store
        .as_ref()
        .unwrap()
        .get_active_plan("test-session", &project_root)
        .unwrap();
    assert!(
        active.is_none(),
        "plan should no longer be active after all tasks completed"
    );
}

#[test]
fn task_block_updates_status() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let (plan_id, _) = setup_active_plan(&mut runtime, &project_root);

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::TaskBlock {
            step: 1,
            reason: Some("dependency missing".to_string()),
        },
    );

    let tasks = runtime
        .task_store
        .as_ref()
        .unwrap()
        .list_tasks(&plan_id)
        .unwrap();
    assert_eq!(tasks[0].status, TaskStatus::Blocked);
    assert_eq!(tasks[0].result_summary, "dependency missing");

    let has_msg = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("blocked")));
    assert!(has_msg, "expected 'blocked' message, got: {events:?}");
}

#[test]
fn task_status_shows_active_plan_tasks() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    setup_active_plan(&mut runtime, &project_root);

    let events = collect_events(&mut runtime, RuntimeRequest::TaskStatus);

    let has_goal = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("test goal")));
    assert!(
        has_goal,
        "expected plan goal in status output, got: {events:?}"
    );

    let has_step = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("First step")));
    assert!(
        has_step,
        "expected step title in status output, got: {events:?}"
    );
}

#[test]
fn task_status_no_active_plan_emits_message() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let mut runtime = make_runtime_in(vec!["unused"], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    let events = collect_events(&mut runtime, RuntimeRequest::TaskStatus);

    let has_msg = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(m) if m.contains("no active plan")));
    assert!(
        has_msg,
        "expected 'no active plan' message, got: {events:?}"
    );
}

// Parse tests

#[test]
fn parses_task_execute() {
    assert_eq!(
        crate::tui::commands::parse("/task 3"),
        Some(Ok(crate::tui::commands::Command::TaskExecute { step: 3 }))
    );
}

#[test]
fn parses_task_status_explicit() {
    assert_eq!(
        crate::tui::commands::parse("/task status"),
        Some(Ok(crate::tui::commands::Command::TaskStatus))
    );
}

#[test]
fn parses_task_bare_defaults_to_status() {
    assert_eq!(
        crate::tui::commands::parse("/task"),
        Some(Ok(crate::tui::commands::Command::TaskStatus))
    );
}

#[test]
fn parses_task_complete() {
    assert_eq!(
        crate::tui::commands::parse("/task complete 2"),
        Some(Ok(crate::tui::commands::Command::TaskComplete {
            step: 2,
            summary: None,
        }))
    );
}

#[test]
fn parses_task_complete_with_summary() {
    assert_eq!(
        crate::tui::commands::parse("/task complete 2 done"),
        Some(Ok(crate::tui::commands::Command::TaskComplete {
            step: 2,
            summary: Some("done".to_string()),
        }))
    );
}

#[test]
fn parses_task_block() {
    assert_eq!(
        crate::tui::commands::parse("/task block 1"),
        Some(Ok(crate::tui::commands::Command::TaskBlock {
            step: 1,
            reason: None,
        }))
    );
}

#[test]
fn parses_task_block_with_reason() {
    assert_eq!(
        crate::tui::commands::parse("/task block 1 missing dep"),
        Some(Ok(crate::tui::commands::Command::TaskBlock {
            step: 1,
            reason: Some("missing dep".to_string()),
        }))
    );
}

#[test]
fn parses_task_non_integer_returns_error() {
    assert!(matches!(
        crate::tui::commands::parse("/task foo"),
        Some(Err(_))
    ));
}
