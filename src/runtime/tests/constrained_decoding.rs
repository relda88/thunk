use super::*;

/// Plan generation uses backend.generate() directly from plan_handlers.rs,
/// bypassing run_generate_turn(). When constrained_output is enabled,
/// constrained_mode must still be None for one-shot synthesis calls.
#[test]
fn plan_generation_never_sets_constrained_mode() {
    let valid_plan = "1. Setup: Initialize the project\n\
                      2. Build: Compile all targets\n\
                      3. Test: Run the full test suite";
    let (rt, requests) = make_runtime_with_recorded_requests(vec![valid_plan]);
    let mut rt = rt.with_constrained_output();
    collect_events(
        &mut rt,
        RuntimeRequest::PlanCreate {
            goal: "ship it".to_string(),
        },
    );

    let requests = requests.lock().unwrap();
    let req = requests.first().expect("backend request must be recorded");
    assert!(
        req.constrained_mode == ConstrainedMode::None,
        "plan generation must not set constrained_mode — \
         it bypasses run_generate_turn() and must never constrain output"
    );
}

/// When constrained_output is enabled, a normal investigation turn
/// (going through run_generate_turn) sets constrained_mode to ToolCall.
#[test]
fn investigation_turn_sets_tool_call_when_constrained_enabled() {
    let (rt, requests) = make_runtime_with_recorded_requests(vec!["Done."]);
    let mut rt = rt.with_constrained_output();
    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "what does main do".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let req = requests.first().expect("backend request must be recorded");
    assert!(
        req.constrained_mode == ConstrainedMode::ToolCall,
        "investigation turn must set constrained_mode::ToolCall when constrained_output is enabled"
    );
}

/// When constrained_output is disabled (default), constrained_mode is None
/// even for investigation turns.
#[test]
fn investigation_turn_has_no_constrained_mode_when_disabled() {
    let (mut rt, requests) = make_runtime_with_recorded_requests(vec!["Done."]);
    collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "what does main do".into(),
        },
    );

    let requests = requests.lock().unwrap();
    let req = requests.first().expect("backend request must be recorded");
    assert!(
        req.constrained_mode == ConstrainedMode::None,
        "investigation turn must not set constrained_mode when constrained_output is disabled"
    );
}
