use super::*;

/// Plan generation uses backend.generate() directly from plan_handlers.rs,
/// bypassing run_generate_turn(). When constrained_output is enabled,
/// tool_call_mode must still be false for one-shot synthesis calls.
#[test]
fn plan_generation_never_sets_tool_call_mode() {
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
        !req.tool_call_mode,
        "plan generation must not set tool_call_mode: true — \
         it bypasses run_generate_turn() and must never constrain output"
    );
}

/// When constrained_output is enabled, a normal investigation turn
/// (going through run_generate_turn) sets tool_call_mode: true.
#[test]
fn investigation_turn_sets_tool_call_mode_when_constrained_enabled() {
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
        req.tool_call_mode,
        "investigation turn must set tool_call_mode: true when constrained_output is enabled"
    );
}

/// When constrained_output is disabled (default), tool_call_mode is false
/// even for investigation turns.
#[test]
fn investigation_turn_has_no_tool_call_mode_when_constrained_disabled() {
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
        !req.tool_call_mode,
        "investigation turn must not set tool_call_mode when constrained_output is disabled"
    );
}
