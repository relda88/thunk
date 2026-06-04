use rusqlite::Connection;

use crate::storage::session::schema;
use crate::storage::tasks::{PlanStatus, TaskStore};

use super::*;

fn attach_task_store_to(runtime: &mut Runtime, path: &std::path::Path) {
    let init_conn = Connection::open(path).unwrap();
    schema::initialize(&init_conn).unwrap();
    drop(init_conn);
    let store = TaskStore::open(path).unwrap();
    runtime.task_store = Some(store);
}

#[test]
fn plan_create_with_valid_response_fires_approval() {
    let valid_plan = "1. Setup: Initialize the project\n\
                      2. Build: Compile all targets\n\
                      3. Test: Run the full test suite";
    let mut runtime = make_runtime(vec![valid_plan]);
    let events = collect_events(
        &mut runtime,
        RuntimeRequest::PlanCreate {
            goal: "ship it".to_string(),
        },
    );
    let approval = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. }));
    assert!(approval.is_some(), "expected PlanApprovalRequired event");
    if let Some(RuntimeEvent::PlanApprovalRequired { goal, steps }) = approval {
        assert_eq!(goal, "ship it");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].0, "Setup");
        assert_eq!(steps[1].0, "Build");
        assert_eq!(steps[2].0, "Test");
    }
}

#[test]
fn plan_create_retries_on_invalid_response() {
    let freeform = "Here is my plan: do things and more things";
    let valid_plan = "1. First: Do the first thing\n2. Second: Do the second thing";
    let (mut runtime, requests) = make_runtime_with_recorded_requests(vec![freeform, valid_plan]);
    let events = collect_events(
        &mut runtime,
        RuntimeRequest::PlanCreate {
            goal: "test retry".to_string(),
        },
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "backend should be called twice (initial + retry)"
    );
    let approval = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. }));
    assert!(
        approval.is_some(),
        "expected PlanApprovalRequired after retry"
    );
}

#[test]
fn plan_create_fails_after_two_bad_responses() {
    let freeform1 = "Here is my plan: step 1 do things";
    let freeform2 = "Just a general approach: iterate and improve";
    let mut runtime = make_runtime(vec![freeform1, freeform2]);
    let events = collect_events(
        &mut runtime,
        RuntimeRequest::PlanCreate {
            goal: "always fail".to_string(),
        },
    );
    let has_approval = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. }));
    assert!(
        !has_approval,
        "should not fire PlanApprovalRequired on two bad responses"
    );

    let system_msgs: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::SystemMessage(m) = e {
                Some(m.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        system_msgs.iter().any(|m| m.contains("could not parse")),
        "expected 'could not parse' error message, got: {system_msgs:?}"
    );
}

#[test]
fn plan_create_blocked_when_active_plan_exists() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let valid_plan = "1. First: Do the first thing\n2. Second: Do the second thing";
    let mut runtime = make_runtime_in(vec![valid_plan], tmp_root.path());
    attach_task_store_to(&mut runtime, tmp_db.path());

    // The runtime canonicalizes the path; match that here.
    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    {
        let store = runtime.task_store.as_ref().unwrap();
        let id = store
            .create_plan("test-session", &project_root, "existing goal")
            .unwrap();
        store.update_plan_status(&id, PlanStatus::Active).unwrap();
    }

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::PlanCreate {
            goal: "new goal".to_string(),
        },
    );
    let has_approval = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. }));
    assert!(
        !has_approval,
        "should not fire PlanApprovalRequired when plan is active"
    );

    let system_msgs: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::SystemMessage(m) = e {
                Some(m.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        system_msgs.iter().any(|m| m.contains("already active")),
        "expected 'already active' message, got: {system_msgs:?}"
    );
}

#[test]
fn plan_approve_persists_to_task_store() {
    let tmp_root = tempfile::TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let valid_plan = "1. Setup: Initialize the project\n2. Build: Compile all targets";
    let mut runtime = make_runtime_in(vec![valid_plan], tmp_root.path());
    attach_task_store_to(&mut runtime, tmp_db.path());

    // The runtime canonicalizes the path; match that here.
    let project_root = tmp_root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // Create the pending plan via PlanCreate.
    let create_events = collect_events(
        &mut runtime,
        RuntimeRequest::PlanCreate {
            goal: "approve test".to_string(),
        },
    );
    assert!(
        create_events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. })),
        "expected PlanApprovalRequired after PlanCreate"
    );

    // Approve it.
    collect_events(&mut runtime, RuntimeRequest::PlanApprove);

    // Verify it's now in the store as active.
    let store = runtime.task_store.as_ref().unwrap();
    let active = store
        .get_active_plan("test-session", &project_root)
        .unwrap();
    assert!(
        active.is_some(),
        "expected active plan in store after PlanApprove"
    );
    let plan = active.unwrap();
    assert_eq!(plan.goal, "approve test");

    let tasks = store.list_tasks(&plan.id).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].title, "Setup");
    assert_eq!(tasks[1].title, "Build");
}
