use rusqlite::Connection;
use tempfile::TempDir;

use crate::storage::session::schema;
use crate::storage::tasks::TaskStore;

use super::*;

fn attach_task_store(runtime: &mut Runtime, path: &std::path::Path) {
    let init_conn = Connection::open(path).unwrap();
    schema::initialize(&init_conn).unwrap();
    drop(init_conn);
    let store = TaskStore::open(path).unwrap();
    runtime.task_store = Some(store);
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
fn agent_run_review_activates_ability_in_request() {
    let (mut rt, requests) = make_runtime_with_recorded_requests(vec!["Review complete."]);

    collect_events(
        &mut rt,
        RuntimeRequest::AgentRun {
            ability: "review".into(),
            target: None,
        },
    );

    let requests = requests.lock().unwrap();
    let first = requests.first().expect("backend request must be recorded");

    let has_ability_marker = first
        .messages
        .iter()
        .any(|m| m.content.contains("[ability: review]"));
    assert!(
        has_ability_marker,
        "expected '[ability: review]' in generation request messages; got: {:?}",
        first
            .messages
            .iter()
            .map(|m| &m.content)
            .collect::<Vec<_>>()
    );
}

#[test]
fn agent_run_restores_previous_ability_after_workflow() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec!["Done."], tmp.path());

    // Activate review ability as the session baseline.
    collect_events(
        &mut rt,
        RuntimeRequest::AbilityToggle {
            name: Some("review".into()),
        },
    );

    // Run the investigate agent workflow — should temporarily override ability.
    collect_events(
        &mut rt,
        RuntimeRequest::AgentRun {
            ability: "investigate".into(),
            target: None,
        },
    );

    // Query current ability — must have reverted to review.
    let events = collect_events(&mut rt, RuntimeRequest::AbilityToggle { name: None });
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("ability: review")),
        "expected ability restored to 'review' after agent workflow, got: {msgs:?}"
    );
}

#[test]
fn agent_run_unsupported_ability_emits_error_and_no_answer() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec!["should not be called"], tmp.path());

    let events = collect_events(
        &mut rt,
        RuntimeRequest::AgentRun {
            ability: "debug".into(),
            target: None,
        },
    );

    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("unsupported ability")),
        "expected 'unsupported ability' error message, got: {msgs:?}"
    );

    let has_answer = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::AnswerReady(_)));
    assert!(
        !has_answer,
        "unsupported ability must not trigger the turn loop; got AnswerReady"
    );
}

#[test]
fn parses_agent_review() {
    use crate::tui::commands::{parse, Command};
    let result = parse("/agent review");
    assert_eq!(
        result,
        Some(Ok(Command::Agent {
            ability: "review".to_string(),
            target: None,
        }))
    );
}

#[test]
fn parses_agent_with_target() {
    use crate::tui::commands::{parse, Command};
    let result = parse("/agent investigate auth service");
    assert_eq!(
        result,
        Some(Ok(Command::Agent {
            ability: "investigate".to_string(),
            target: Some("auth service".to_string()),
        }))
    );
}

#[test]
fn parses_agent_missing_ability() {
    use crate::tui::commands::{parse, ParseError};
    let result = parse("/agent");
    assert_eq!(
        result,
        Some(Err(ParseError::MissingArgument { command: "/agent" }))
    );
}

#[test]
fn agent_run_refactor_emits_plan_approval_required() {
    let tmp_root = TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    // The investigation turn issues a SEARCH_BEFORE_ANSWERING correction (one extra
    // backend call) before emitting the terminal answer, so three responses are needed:
    // [0] investigation, [1] correction-round filler, [2] plan steps for generate_plan_steps.
    let investigation_answer = "Investigation complete. The module has mixed concerns.";
    let correction_filler = "I will search the codebase now.";
    let plan_steps = "1. Extract: Move validation logic to a separate module\n\
                      2. Decouple: Remove direct dependency on file store";
    let mut runtime = make_runtime_in(
        vec![investigation_answer, correction_filler, plan_steps],
        tmp_root.path(),
    );
    attach_task_store(&mut runtime, tmp_db.path());

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::AgentRun {
            ability: "refactor".into(),
            target: Some("sandbox/services/task_service.py".into()),
        },
    );

    let approval = events
        .iter()
        .find(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. }));
    assert!(
        approval.is_some(),
        "expected PlanApprovalRequired after agent refactor workflow; got: {events:?}"
    );
    if let Some(RuntimeEvent::PlanApprovalRequired { goal, steps }) = approval {
        assert!(
            goal.contains("task_service.py"),
            "expected goal to reference target; got: {goal:?}"
        );
        assert_eq!(steps.len(), 2, "expected 2 plan steps; got: {steps:?}");
        assert_eq!(steps[0].0, "Extract");
        assert_eq!(steps[1].0, "Decouple");
    }
}

#[test]
fn agent_run_refactor_restores_ability_after_workflow() {
    let tmp_root = TempDir::new().unwrap();
    let tmp_db = tempfile::NamedTempFile::new().unwrap();

    let investigation_answer = "Found mixed concerns.";
    let plan_steps = "1. Extract: Pull out the concern\n2. Rename: Use clear names";
    let mut runtime = make_runtime_in(vec![investigation_answer, plan_steps], tmp_root.path());
    attach_task_store(&mut runtime, tmp_db.path());

    // Activate review ability as the session baseline.
    collect_events(
        &mut runtime,
        RuntimeRequest::AbilityToggle {
            name: Some("review".into()),
        },
    );

    collect_events(
        &mut runtime,
        RuntimeRequest::AgentRun {
            ability: "refactor".into(),
            target: None,
        },
    );

    // Ability must have reverted to review.
    let events = collect_events(&mut runtime, RuntimeRequest::AbilityToggle { name: None });
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("ability: review")),
        "expected ability restored to 'review' after agent refactor workflow, got: {msgs:?}"
    );
}

#[test]
fn agent_run_refactor_no_storage_emits_error() {
    let tmp_root = TempDir::new().unwrap();
    let mut runtime = make_runtime_in(vec!["Investigation done."], tmp_root.path());
    // No task store attached.

    let events = collect_events(
        &mut runtime,
        RuntimeRequest::AgentRun {
            ability: "refactor".into(),
            target: Some("src/foo.rs".into()),
        },
    );

    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("no storage configured")),
        "expected 'no storage configured' error; got: {msgs:?}"
    );
    let has_approval = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::PlanApprovalRequired { .. }));
    assert!(
        !has_approval,
        "must not fire PlanApprovalRequired without storage; got approval"
    );
}
