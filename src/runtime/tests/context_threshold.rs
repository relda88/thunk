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

/// Run a submit turn and return all emitted events.
fn submit(runtime: &mut Runtime, prompt: &str) -> Vec<RuntimeEvent> {
    collect_events(
        runtime,
        RuntimeRequest::Submit {
            text: prompt.to_string(),
        },
    )
}

#[test]
fn warning_fires_at_75_pct() {
    // context_window = 100 tokens; backend reports 80 prompt tokens → 80% → warning
    let mut rt = make_runtime_with_token_counting_backend(vec!["answer"], 80, Some(100));
    let events = submit(&mut rt, "hello");
    let msgs = system_messages(&events);
    assert!(
        msgs.iter()
            .any(|m| m.contains("context at 75%") && m.contains("/compact")),
        "75%% warning must fire when pct >= 75: {msgs:?}"
    );
}

#[test]
fn warning_does_not_fire_below_75_pct() {
    // 74 tokens of 100 → 74% → no warning
    let mut rt = make_runtime_with_token_counting_backend(vec!["answer"], 74, Some(100));
    let events = submit(&mut rt, "hello");
    let msgs = system_messages(&events);
    assert!(
        !msgs.iter().any(|m| m.contains("context at 75%")),
        "warning must not fire when pct < 75: {msgs:?}"
    );
}

#[test]
fn auto_prune_fires_at_90_pct() {
    // 95 tokens of 100 → 95% → auto-prune attempted.
    // With a fresh conversation there's nothing stale to prune, so no notice is emitted
    // (the compact returns 0). The important thing is the code path is exercised.
    let mut rt = make_runtime_with_token_counting_backend(vec!["answer"], 95, Some(100));
    let events = submit(&mut rt, "hello");
    // At 95% the logic enters the ≥90 branch. Since there are no stale tool results in a
    // fresh session the notice is silently skipped, but context_75_warned must be set
    // (verified by checking the warning does NOT also appear).
    let msgs = system_messages(&events);
    assert!(
        !msgs.iter().any(|m| m.contains("context at 75%")),
        "75%% warning must not appear separately when pct >= 90: {msgs:?}"
    );
}

#[test]
fn warning_fires_only_once_per_session() {
    let mut rt = make_runtime_with_token_counting_backend(vec!["answer", "answer"], 80, Some(100));

    let events1 = submit(&mut rt, "turn one");
    let msgs1 = system_messages(&events1);
    assert!(
        msgs1.iter().any(|m| m.contains("context at 75%")),
        "warning must fire on first crossing: {msgs1:?}"
    );

    let events2 = submit(&mut rt, "turn two");
    let msgs2 = system_messages(&events2);
    assert!(
        !msgs2.iter().any(|m| m.contains("context at 75%")),
        "warning must not fire again on second turn: {msgs2:?}"
    );
}

#[test]
fn reset_clears_context_75_warned_flag() {
    let mut rt = make_runtime_with_token_counting_backend(vec!["answer", "answer"], 80, Some(100));

    // First turn — warning fires
    let events1 = submit(&mut rt, "turn one");
    assert!(
        system_messages(&events1)
            .iter()
            .any(|m| m.contains("context at 75%")),
        "warning must fire before reset"
    );

    // Reset clears the flag
    rt.handle(RuntimeRequest::Reset, &mut |_| {});

    // Second turn — warning fires again because flag was cleared
    let events2 = submit(&mut rt, "turn two");
    assert!(
        system_messages(&events2)
            .iter()
            .any(|m| m.contains("context at 75%")),
        "warning must fire again after reset"
    );
}

#[test]
fn no_warning_when_no_context_window_configured() {
    // context_window_tokens = None → context_used_pct returns None → no warning
    let mut rt = make_runtime_with_token_counting_backend(vec!["answer"], 80, None);
    let events = submit(&mut rt, "hello");
    let msgs = system_messages(&events);
    assert!(
        !msgs.iter().any(|m| m.contains("context at 75%")),
        "warning must not fire when no context window is configured: {msgs:?}"
    );
}
