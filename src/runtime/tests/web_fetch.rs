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
fn fetch_private_ip_emits_system_error() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::FetchUrl {
            url: "http://127.0.0.1/".into(),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("not permitted")),
        "expected 'not permitted' in system messages, got: {msgs:?}"
    );
}

#[test]
fn fetch_invalid_url_emits_system_error() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::FetchUrl {
            url: "not-a-url".into(),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("fetch:")),
        "expected 'fetch:' error in system messages, got: {msgs:?}"
    );
}

#[test]
fn fetch_ftp_scheme_emits_system_error() {
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::FetchUrl {
            url: "ftp://example.com/file".into(),
        },
    );
    let msgs = system_messages(&events);
    assert!(
        msgs.iter().any(|m| m.contains("fetch:")),
        "expected 'fetch:' error in system messages, got: {msgs:?}"
    );
}
