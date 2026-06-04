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
