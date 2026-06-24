use super::super::investigation::anchors::{
    has_same_scope_reference, is_last_search_anchor_prompt, AnchorState,
};
use super::super::investigation::investigation::{InvestigationMode, InvestigationState};
use super::super::investigation::tool_surface::ToolSurface;
use super::super::lsp::LspManager;
use super::super::orchestration::context_cap::cap_tool_result_blocks;
use super::super::orchestration::tool_round::{run_tool_round, SearchBudget, ToolRoundOutcome};
use super::super::protocol::response_text::*;
use super::super::types::RuntimeTerminalReason;
use super::*;
use crate::core::config::{Config, LspConfig, RetrievalConfig};
use crate::llm::backend::{BackendCapabilities, BackendEvent, GenerateRequest, ModelBackend};
use crate::runtime::ProjectRoot;
use crate::tools::{default_registry, ToolInput};

struct TestBackend {
    responses: Vec<String>,
    call_count: usize,
}

impl TestBackend {
    fn new(responses: Vec<impl Into<String>>) -> Self {
        Self {
            responses: responses.into_iter().map(Into::into).collect(),
            call_count: 0,
        }
    }
}

impl ModelBackend for TestBackend {
    fn name(&self) -> &str {
        "test"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            context_window_tokens: None,
            max_output_tokens: None,
        }
    }

    fn generate(
        &mut self,
        _request: GenerateRequest,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> crate::core::error::Result<()> {
        let reply = self
            .responses
            .get(self.call_count)
            .cloned()
            .unwrap_or_default();
        self.call_count += 1;
        if !reply.is_empty() {
            on_event(BackendEvent::TextDelta(reply));
        }
        on_event(BackendEvent::Finished);
        Ok(())
    }
}

fn make_runtime_in(responses: Vec<impl Into<String>>, root: &std::path::Path) -> Runtime {
    let project_root = ProjectRoot::new(root.to_path_buf()).unwrap();
    Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(TestBackend::new(responses)),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-session".to_string(),
    )
}

fn collect_events(runtime: &mut Runtime, request: RuntimeRequest) -> Vec<RuntimeEvent> {
    let mut events = Vec::new();
    runtime.handle(request, &mut |e| events.push(e));
    events
}

fn has_failed(events: &[RuntimeEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::Failed { .. }))
}

#[test]
fn raw_direct_read_returns_file_contents_without_synthesis_round() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::write(
        tmp.path().join("sandbox/services/task_service.py"),
        "def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(vec!["THIS SHOULD NOT APPEAR"], tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Read sandbox/services/task_service.py".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    let assistant_messages: Vec<&str> = snapshot
        .iter()
        .filter(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(assistant_messages.len(), 1);
    assert!(
        assistant_messages[0].contains("def filtered_tasks(tasks):")
            && assistant_messages[0].contains("return [task for task in tasks if task.completed]"),
        "raw direct read must finalize with file contents only: {assistant_messages:?}"
    );
    assert!(
        snapshot
            .iter()
            .all(|m| !m.content.contains("THIS SHOULD NOT APPEAR")),
        "raw direct read must not consume a synthesis response"
    );
}

#[test]
fn explain_direct_read_reads_then_synthesizes() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::write(
        tmp.path().join("sandbox/services/task_service.py"),
        "def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]\n",
    )
    .unwrap();

    let final_answer = "This file filters completed tasks from the input list.";
    let mut rt = make_runtime_in(vec![final_answer], tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Explain sandbox/services/task_service.py".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: read_file ===")),
        "explain direct read must commit the seeded read result"
    );
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(last_assistant, Some(final_answer));
    assert_ne!(
        last_assistant,
        Some("def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]"),
        "explain direct read must not fall back to raw file contents"
    );
}

#[test]
fn what_does_direct_read_behaves_like_explain() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::write(
        tmp.path().join("sandbox/services/task_service.py"),
        "def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]\n",
    )
    .unwrap();

    let final_answer = "This file defines logic for filtering completed tasks.";
    let mut rt = make_runtime_in(vec![final_answer], tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "What does sandbox/services/task_service.py do?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: read_file ===")),
        "what-does direct read must commit the seeded read result"
    );
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(last_assistant, Some(final_answer));
}

#[test]
fn what_does_bare_filename_seeds_read_before_generation() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::write(
        tmp.path().join("sandbox/services/task_service.py"),
        "def filtered_tasks(tasks): pass\n",
    )
    .unwrap();

    // The backend receives no synthesizable responses — the turn will eventually
    // terminate on an evidence guard. What we verify is that read_file is the
    // very first tool the runtime calls (i.e., the seeded pre-generation direct
    // read fired before any model generation round).
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "What does task_service.py do?".into(),
        },
    );

    let first_tool = events.iter().find_map(|e| {
        if let RuntimeEvent::ToolCallStarted { name } = e {
            Some(name.as_str())
        } else {
            None
        }
    });
    assert_eq!(
        first_tool,
        Some("read_file"),
        "bare filename must seed read_file as the first tool call; events: {events:?}"
    );

    // The seeded read result must appear in the conversation before any
    // generation — confirmed by the tool_result block being committed.
    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot
            .iter()
            .any(|m| m.content.contains("=== tool_result: read_file ===")),
        "read_file tool_result must be committed to conversation; snapshot: {snapshot:?}"
    );
}

#[test]
fn explain_direct_read_repeated_tool_fallback_does_not_dump_file_contents() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::write(
        tmp.path().join("sandbox/services/task_service.py"),
        "def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[read_file: sandbox/services/task_service.py]",
            "[read_file: sandbox/services/task_service.py]",
        ],
        tmp.path(),
    );
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Explain sandbox/services/task_service.py".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "turn must terminate cleanly: {events:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some(repeated_tool_after_answer_phase_final_answer())
    );
    assert_ne!(
        last_assistant,
        Some("def filtered_tasks(tasks):\n    return [task for task in tasks if task.completed]"),
        "explain-mode repeated-tool fallback must not dump raw file contents"
    );
}

// cap_tool_result_blocks tests

#[test]
fn cap_under_limit_is_noop() {
    let text = "=== tool_result: read_file ===\nline1\nline2\n=== /tool_result ===\n\n";
    assert_eq!(cap_tool_result_blocks(text, 5), text);
}

#[test]
fn cap_over_limit_truncates_and_adds_note() {
    let body_lines: Vec<String> = (1..=5).map(|i| format!("line{i}")).collect();
    let body = body_lines.join("\n") + "\n";
    let text = format!("=== tool_result: read_file ===\n{body}=== /tool_result ===\n\n");
    let result = cap_tool_result_blocks(&text, 3);
    assert!(
        result.contains("line1\nline2\nline3\n"),
        "first 3 lines must be kept"
    );
    assert!(!result.contains("line4"), "line4 must be removed");
    assert!(result.contains("[capped at 3 lines — original: 5 lines]"));
    assert!(result.contains("=== tool_result: read_file ==="));
    assert!(result.contains("=== /tool_result ==="));
}

#[test]
fn cap_leaves_non_tool_result_content_unchanged() {
    let text = "[runtime:correction] must not fabricate tool calls\n";
    assert_eq!(cap_tool_result_blocks(text, 5), text);
}

#[test]
fn cap_processes_multi_block_independently() {
    let block = |n: usize| {
        let body: String = (1..=n).map(|i| format!("line{i}\n")).collect();
        format!("=== tool_result: read_file ===\n{body}=== /tool_result ===\n\n")
    };
    // Two blocks, both over the limit of 2
    let text = format!("{}{}", block(4), block(3));
    let result = cap_tool_result_blocks(&text, 2);
    assert_eq!(result.matches("[capped at 2 lines").count(), 2);
}

#[test]
fn cap_error_blocks_pass_through_unchanged() {
    let text = "=== tool_error: read_file ===\nfile not found\n=== /tool_error ===\n\n";
    assert_eq!(cap_tool_result_blocks(text, 1), text);
}

#[test]
fn search_anchor_stores_effective_clamped_scope() {
    use std::collections::HashSet;
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox")).unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("sandbox/in_scope.py"), "needle = True\n").unwrap();
    fs::write(tmp.path().join("src/outside.py"), "needle = False\n").unwrap();

    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let registry = default_registry().with_project_root(project_root.as_path_buf());
    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed_tool_attempts = 0usize;
    let mut weak_search_query_attempts = 0usize;
    let mut events = Vec::new();

    let outcome = run_tool_round(
        &project_root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "needle".into(),
            path: Some("src/".into()),
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed_tool_attempts,
        &mut weak_search_query_attempts,
        false,
        true,
        InvestigationMode::UsageLookup,
        None,
        &mut requested_read_completed,
        Some("sandbox/"),
        None,
        None,
        &RetrievalConfig::default(),
        &mut |e| events.push(e),
    );

    match outcome {
        ToolRoundOutcome::RuntimeDispatch {
            call: ToolInput::ReadFile { path },
            ..
        } => assert!(
            path.ends_with("sandbox/in_scope.py"),
            "usage lookup should auto-read the in-scope preferred candidate: {path}"
        ),
        _ => panic!("usage lookup search should now runtime-dispatch a preferred read"),
    }
    assert_eq!(anchors.last_search_query(), Some("needle"));
    assert_eq!(anchors.last_search_scope(), Some("sandbox/"));
}

#[test]
fn failed_search_code_does_not_update_last_search_anchor() {
    use std::collections::HashSet;
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.rs"), "fn needle() {}\n").unwrap();
    fs::create_dir_all(tmp.path().join("sandbox")).unwrap();
    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let registry = default_registry().with_project_root(project_root.as_path_buf());
    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut reads_this_turn = HashSet::new();
    let mut anchors = AnchorState::default();
    let mut requested_read_completed = false;
    let mut disallowed_tool_attempts = 0usize;
    let mut weak_search_query_attempts = 0usize;
    let mut events = Vec::new();

    let seed_outcome = run_tool_round(
        &project_root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "needle".into(),
            path: Some("sandbox/".into()),
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed_tool_attempts,
        &mut weak_search_query_attempts,
        false,
        false,
        InvestigationMode::General,
        None,
        &mut requested_read_completed,
        None,
        None,
        None,
        &RetrievalConfig::default(),
        &mut |e| events.push(e),
    );
    assert!(
        matches!(seed_outcome, ToolRoundOutcome::Completed { .. }),
        "seed search round must complete"
    );
    assert_eq!(anchors.last_search_query(), Some("needle"));
    assert_eq!(anchors.last_search_scope(), Some("sandbox/"));

    let outcome = run_tool_round(
        &project_root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "".into(),
            path: None,
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed_tool_attempts,
        &mut weak_search_query_attempts,
        false,
        false,
        InvestigationMode::General,
        None,
        &mut requested_read_completed,
        None,
        None,
        None,
        &RetrievalConfig::default(),
        &mut |e| events.push(e),
    );

    assert!(
        matches!(outcome, ToolRoundOutcome::Completed { .. }),
        "failed non-read tool should return completed with tool error"
    );
    assert_eq!(anchors.last_search_query(), Some("needle"));
    assert_eq!(anchors.last_search_scope(), Some("sandbox/"));
}
#[test]
fn unsupported_search_anchor_phrases_do_not_resolve() {
    assert!(!is_last_search_anchor_prompt("search it again"));
    assert!(!is_last_search_anchor_prompt("search for that thing again"));
    assert!(!is_last_search_anchor_prompt("search again"));
    assert!(is_last_search_anchor_prompt("search that again"));
    assert!(is_last_search_anchor_prompt("repeat the last search"));
}

#[test]
fn same_scope_followup_after_empty_scope_search_fails_deterministically() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());
    let output =
        crate::tools::ToolOutput::SearchResults(crate::tools::types::SearchResultsOutput {
            query: "needle".into(),
            matches: Vec::new(),
            total_matches: 0,
            truncated: false,
        });

    rt.anchors
        .record_successful_search(&output, "needle".into(), Some("   ".into()));
    assert_eq!(rt.anchors.last_search_query(), Some("needle"));
    assert_eq!(rt.anchors.last_search_scope(), None);
    assert_eq!(rt.anchors.last_scoped_search_scope(), None);

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where database is configured in the same folder".into(),
        },
    );

    assert!(
        events.iter().any(|e| matches!(
            e,
            RuntimeEvent::AssistantMessageChunk(chunk)
                if chunk == NO_LAST_SCOPED_SEARCH_AVAILABLE
        )),
        "empty stored scope must not provide same-scope continuity: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ToolCallStarted { .. })),
        "empty stored scope must not dispatch tools: {events:?}"
    );
}

#[test]
fn unsupported_same_scope_phrases_do_not_match() {
    assert!(!has_same_scope_reference("Find database in the same place"));
    assert!(!has_same_scope_reference("Find it there"));
    assert!(!has_same_scope_reference("Search the same place"));
    assert!(!has_same_scope_reference("Find database in this folder"));
    assert!(!has_same_scope_reference(
        "Find database in the same folderish"
    ));
    assert!(!has_same_scope_reference(
        "Find database within the same scopekeeper"
    ));
    assert!(has_same_scope_reference("Find database in the same folder"));
    assert!(has_same_scope_reference(
        "Find database within the same directory"
    ));
    assert!(has_same_scope_reference(
        "Find database within the same scope"
    ));
}

#[test]
fn same_scope_forced_broader_path_clamps_to_prior_scoped_search() {
    use std::collections::HashSet;
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(
        tmp.path().join("sandbox/services/logging.py"),
        "def initialize_logging():\n    pass\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/services/database.yaml"),
        "database: sqlite:///service.db\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("src/database.yaml"),
        "database: sqlite:///wrong.db\n",
    )
    .unwrap();

    let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
    let registry = default_registry().with_project_root(project_root.as_path_buf());
    let mut anchors = AnchorState::default();
    let mut events = Vec::new();

    let mut seed_last_call_key = None;
    let mut seed_search_budget = SearchBudget::new();
    let mut seed_investigation = InvestigationState::new();
    let mut seed_reads_this_turn = HashSet::new();
    let mut seed_requested_read_completed = false;
    let mut seed_disallowed_tool_attempts = 0usize;
    let mut seed_weak_search_query_attempts = 0usize;
    let seed_outcome = run_tool_round(
        &project_root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "logging".into(),
            path: Some("sandbox/services/".into()),
        }],
        &mut seed_last_call_key,
        &mut seed_search_budget,
        &mut seed_investigation,
        &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
        &mut seed_reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut seed_disallowed_tool_attempts,
        &mut seed_weak_search_query_attempts,
        false,
        true,
        InvestigationMode::InitializationLookup,
        None,
        &mut seed_requested_read_completed,
        None,
        None,
        None,
        &RetrievalConfig::default(),
        &mut |e| events.push(e),
    );
    assert!(
        matches!(seed_outcome, ToolRoundOutcome::Completed { .. }),
        "seed scoped search must complete"
    );
    assert_eq!(
        anchors.last_scoped_search_scope(),
        Some("sandbox/services/")
    );

    let same_scope = anchors
        .last_scoped_search_scope()
        .map(str::to_string)
        .expect("seeded scoped search");
    let mut last_call_key = None;
    let mut search_budget = SearchBudget::new();
    let mut investigation = InvestigationState::new();
    let mut reads_this_turn = HashSet::new();
    let mut requested_read_completed = false;
    let mut disallowed_tool_attempts = 0usize;
    let mut weak_search_query_attempts = 0usize;
    let outcome = run_tool_round(
        &project_root,
        &registry,
        vec![ToolInput::SearchCode {
            query: "database".into(),
            path: Some("src/".into()),
        }],
        &mut last_call_key,
        &mut search_budget,
        &mut investigation,
        &mut LspManager::new(&LspConfig::default(), std::path::Path::new(".")),
        &mut reads_this_turn,
        &mut anchors,
        ToolSurface::RetrievalFirst,
        &mut disallowed_tool_attempts,
        &mut weak_search_query_attempts,
        false,
        true,
        InvestigationMode::ConfigLookup,
        None,
        &mut requested_read_completed,
        Some(&same_scope),
        None,
        None,
        &RetrievalConfig::default(),
        &mut |e| events.push(e),
    );

    let results = match outcome {
        ToolRoundOutcome::Completed { results, .. } => results,
        _ => panic!("forced same-scope clamp should complete"),
    };
    assert!(
        results.contains("sandbox/services/database.yaml"),
        "clamped same-scope search must include prior scoped path: {results}"
    );
    assert!(
        !results.contains("src/database.yaml"),
        "broader model path must be clamped away from src/: {results}"
    );
    assert_eq!(
        anchors.last_scoped_search_scope(),
        Some("sandbox/services/")
    );
}

// Phase 9.1.1 — bounded multi-step investigation

#[test]
fn two_candidate_reads_both_insufficient_terminates_cleanly() {
    // Usage lookup: three search candidates (two definition-only + one usage).
    // First read is definition-only → recovery correction fires pointing to usage file.
    // Model ignores correction and reads a second definition-only file.
    // After two candidate reads with evidence still not ready the runtime must
    // terminate cleanly with InsufficientEvidence — no further correction cycles.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("models")).unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("models").join("enums.py"),
        "class TaskStatus(str, Enum):\n    TODO = \"todo\"\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("models").join("alt_enums.py"),
        "class TaskStatus:\n    DONE = \"done\"\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("task_service.py"),
        "from models.enums import TaskStatus\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: TaskStatus]",
            // Round 2: reads first definition file.
            // Runtime auto-dispatches task_service.py (import-only, no usage evidence).
            "[read_file: models/enums.py]",
            // Round 3: model tries second definition file.
            // candidate_reads_count reaches 2 after the auto-dispatch; read is blocked.
            "[read_file: models/alt_enums.py]",
            // Round 4 would be model synthesis — not reached; runtime terminates first.
            "TaskStatus is defined in models/enums.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is TaskStatus used?".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "turn must terminate cleanly: {events:?}"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(
            answer_source,
            Some(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::InsufficientEvidence,
                ..
            })
        ),
        "two insufficient candidate reads must produce InsufficientEvidence: {answer_source:?}"
    );

    // The model's premature synthesis must not appear as the last assistant message.
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some(ungrounded_investigation_final_answer()),
        "last assistant must be the runtime terminal, not model synthesis"
    );
}

#[test]
fn prose_after_search_seeds_read_file_directly() {
    // When the model emits prose immediately after search results without calling
    // read_file, the runtime seeds a read_file call for the best candidate rather
    // than issuing a correction message.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("lib.rs"),
        "pub fn target_fn() { /* impl */ }\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: target_fn]",        // search → finds lib.rs
            "target_fn is in lib.rs.",         // prose without read → runtime seeds read
            "target_fn is defined in lib.rs.", // synthesis after seeded read → accepted
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is target_fn defined?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");

    let snapshot = rt.messages_snapshot();

    let correction_count = snapshot
        .iter()
        .filter(|m| {
            m.content.starts_with("[runtime:correction]")
                && m.content.contains("no matched file has been read")
        })
        .count();
    assert_eq!(
        correction_count, 0,
        "runtime must seed a read directly rather than issuing a correction"
    );

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "seeded read must produce a ToolAssisted answer: {answer_source:?}"
    );
}

// Phase 9.1.2 — Path-Scoped Investigation

// Phase 9.1.4 — Prompt Scope as Search Upper Bound

// Phase 9.1.3 — Candidate Selection Quality (import-only weak candidate rejection)

#[test]
fn config_lookup_second_non_config_candidate_after_recovery_is_not_accepted() {
    // Config lookup: config candidate exists, but the model ignores the config recovery
    // and reads a second non-config candidate. The second read must remain insufficient;
    // after two candidate reads the bounded investigation terminates cleanly.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("config")).unwrap();
    fs::write(
        tmp.path().join("services").join("database.py"),
        "database = os.getenv(\"DATABASE_URL\")\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("database_alt.py"),
        "database = load_from_environment()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("config").join("database.yaml"),
        "database:\n  url: postgres://localhost/mydb\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: database]",
            "[read_file: services/database.py]",
            "[read_file: services/database_alt.py]",
            "The database is configured in config/database.yaml.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is the database configured?".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "turn must terminate cleanly: {events:?}"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to config file must admit synthesis: {answer_source:?}"
    );

    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("The database is configured in config/database.yaml."),
        "last assistant must be the model synthesis from the dispatched config read"
    );
}

// Phase 9.2.2 — Narrow Action-Specific Lookup Satisfaction: Initialization Lookup

#[test]
fn initialization_lookup_second_non_initialization_after_recovery_is_not_accepted() {
    // Initialization lookup: initialization candidate exists, but the model ignores
    // recovery and reads a second non-initialization candidate. That second read must
    // remain insufficient; after two candidate reads the runtime terminates cleanly.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("logging_factory.py"),
        "logger = logging.getLogger(__name__)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("logging_reader.py"),
        "logging.getLogger(\"reader\")\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("logging_setup.py"),
        "def initialize_logging():\n    logging.basicConfig(level=logging.INFO)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: logging]",
            "[read_file: services/logging_factory.py]",
            "[read_file: services/logging_reader.py]",
            "Logging is initialized in services/logging_setup.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where logging is initialized".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "turn must terminate cleanly: {events:?}"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to initialization file must admit synthesis: {answer_source:?}"
    );

    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Logging is initialized in services/logging_setup.py."),
        "last assistant must be the model synthesis from the dispatched initialization read"
    );
}

#[test]
fn initialization_lookup_path_scope_keeps_candidates_inside_scope() {
    // Prompt scope must remain the upper bound. The out-of-scope initialization
    // file is stronger-looking but must not appear in search candidates.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/other")).unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/services")
            .join("logging_factory.py"),
        "logger = logging.getLogger(__name__)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/services").join("logging_setup.py"),
        "def initialize_logging():\n    logging.basicConfig(level=logging.INFO)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/other").join("logging_setup.py"),
        "def initialize_logging():\n    logging.basicConfig(level=logging.DEBUG)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: logging]",
            "[read_file: sandbox/services/logging_factory.py]",
            "[read_file: sandbox/services/logging_setup.py]",
            "Logging is initialized in sandbox/services/logging_setup.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where logging is initialized in sandbox/services/".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    let search_result = snapshot
        .iter()
        .find(|m| m.content.contains("=== tool_result: search_code ==="))
        .map(|m| m.content.as_str())
        .unwrap_or("");
    assert!(
        search_result.contains("sandbox/services/logging_factory.py"),
        "scoped search must include in-scope non-initialization candidate: {search_result}"
    );
    assert!(
        search_result.contains("sandbox/services/logging_setup.py"),
        "scoped search must include in-scope initialization candidate: {search_result}"
    );
    assert!(
        !search_result.contains("sandbox/other/logging_setup.py"),
        "scoped search must exclude out-of-scope initialization candidate: {search_result}"
    );

    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Logging is initialized in sandbox/services/logging_setup.py.")
    );
}

#[test]
fn scoped_final_answer_rejects_out_of_scope_path_before_unread_guard() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/other")).unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/services")
            .join("logging_factory.py"),
        "logger = logging.getLogger(__name__)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/services").join("logging_setup.py"),
        "def initialize_logging():\n    logging.basicConfig(level=logging.INFO)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/other").join("logging_setup.py"),
        "def initialize_logging():\n    logging.basicConfig(level=logging.DEBUG)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: logging]",
            "[read_file: sandbox/services/logging_factory.py]",
            "[read_file: sandbox/services/logging_setup.py]",
            "Logging is initialized in sandbox/other/logging_setup.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where logging is initialized in sandbox/services/".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "turn must terminate cleanly: {events:?}"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(
            answer_source,
            Some(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::InsufficientEvidence,
                ..
            })
        ),
        "out-of-scope final answer must produce InsufficientEvidence: {answer_source:?}"
    );

    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some(
            "The investigation is scoped to `sandbox/services/`, but the answer cited \
                 `sandbox/other/logging_setup.py`. No answer can be given using files outside \
                 the active search scope."
        ),
        "scope guard must fire before the unread-path guard"
    );
}

// Phase 9.2.3 — CreateLookup

// Phase 9.2.4 — RegisterLookup

#[test]
fn register_lookup_path_scope_keeps_candidates_inside_scope() {
    // Prompt scope must remain the upper bound. The out-of-scope registration
    // file is stronger-looking but must not appear in search candidates.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/cli")).unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::write(
        tmp.path().join("sandbox/cli").join("commands.py"),
        "def command_handler(command):\n    return command.run()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/cli").join("registry.py"),
        "def wire_command(command):\n    registry.register(command)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/services").join("registry.py"),
        "def wire_command(command):\n    registry.register(command)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: command]",
            "[read_file: sandbox/cli/commands.py]",
            "[read_file: sandbox/cli/registry.py]",
            "Commands are registered in sandbox/cli/registry.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where commands are registered in sandbox/cli/".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    let search_result = snapshot
        .iter()
        .find(|m| m.content.contains("=== tool_result: search_code ==="))
        .map(|m| m.content.as_str())
        .unwrap_or("");
    assert!(
        search_result.contains("sandbox/cli/commands.py"),
        "scoped search must include in-scope non-register candidate: {search_result}"
    );
    assert!(
        search_result.contains("sandbox/cli/registry.py"),
        "scoped search must include in-scope register candidate: {search_result}"
    );
    assert!(
        !search_result.contains("sandbox/services/registry.py"),
        "scoped search must exclude out-of-scope register candidate: {search_result}"
    );

    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Commands are registered in sandbox/cli/registry.py.")
    );
}

// Phase 9.2.5 — LoadLookup

#[test]
fn load_lookup_path_scope_keeps_candidates_inside_scope() {
    // Prompt scope must remain the upper bound. The out-of-scope load
    // file is stronger-looking but must not appear in search candidates.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/controllers")).unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/services")
            .join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/services")
            .join("session_loader.py"),
        "def get_session(session_id):\n    return load_session(session_id)\n",
    )
    .unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/controllers")
            .join("session_loader.py"),
        "def get_session(session_id):\n    return load_session(session_id)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: sandbox/services/session_handler.py]",
            "[read_file: sandbox/services/session_loader.py]",
            "Sessions are loaded in sandbox/services/session_loader.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where sessions are loaded in sandbox/services/".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    let search_result = snapshot
        .iter()
        .find(|m| m.content.contains("=== tool_result: search_code ==="))
        .map(|m| m.content.as_str())
        .unwrap_or("");
    assert!(
        search_result.contains("sandbox/services/session_handler.py"),
        "scoped search must include in-scope non-load candidate: {search_result}"
    );
    assert!(
        search_result.contains("sandbox/services/session_loader.py"),
        "scoped search must include in-scope load candidate: {search_result}"
    );
    assert!(
        !search_result.contains("sandbox/controllers/session_loader.py"),
        "scoped search must exclude out-of-scope load candidate: {search_result}"
    );

    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are loaded in sandbox/services/session_loader.py.")
    );
}

#[test]
fn load_lookup_read_cap_still_applies() {
    // MaxReadsPerTurn must still apply under LoadLookup.
    // The load file is dispatched after the first non-load read; evidence_ready
    // fires once the load file is read, which bounds further reads via the
    // answer-phase mechanism before the raw per-turn cap is reached.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    for dir in &["a", "b", "c", "d"] {
        fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    fs::write(
        tmp.path().join("a").join("session.py"),
        "def session_a(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("b").join("session.py"),
        "def session_b(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("c").join("session.py"),
        "def session_c(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("d").join("session.py"),
        "session = load_session(session_id)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            // Model reads a non-load file; runtime dispatches the load file, which
            // triggers evidence_ready and bounds remaining reads via answer-phase.
            "[read_file: a/session.py]",
            "[read_file: b/session.py]",
            "[read_file: c/session.py]",
            "[read_file: d/session.py]",
            "Sessions are loaded in d/session.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "must not fail (cap is a correction): {events:?}"
    );
    let snapshot = rt.messages_snapshot();
    let read_count = snapshot
        .iter()
        .filter(|m| m.content.contains("=== tool_result: read_file ==="))
        .count();
    assert!(
        read_count <= 3,
        "reads must be bounded to at most 3 per turn; got {read_count}"
    );
}

// Phase 9.2.6 — SaveLookup

#[test]
fn save_lookup_path_scope_keeps_candidates_inside_scope() {
    // Prompt scope must remain the upper bound. The out-of-scope save
    // file is stronger-looking but must not appear in search candidates.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/services")).unwrap();
    fs::create_dir_all(tmp.path().join("sandbox/controllers")).unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/services")
            .join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("sandbox/services").join("session_store.py"),
        "def store_session(session):\n    save_session(session)\n",
    )
    .unwrap();
    fs::write(
        tmp.path()
            .join("sandbox/controllers")
            .join("session_store.py"),
        "def store_session(session):\n    save_session(session)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: sandbox/services/session_handler.py]",
            "[read_file: sandbox/services/session_store.py]",
            "Sessions are saved in sandbox/services/session_store.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Find where sessions are saved in sandbox/services/".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    let search_result = snapshot
        .iter()
        .find(|m| m.content.contains("=== tool_result: search_code ==="))
        .map(|m| m.content.as_str())
        .unwrap_or("");
    assert!(
        search_result.contains("sandbox/services/session_handler.py"),
        "scoped search must include in-scope non-save candidate: {search_result}"
    );
    assert!(
        search_result.contains("sandbox/services/session_store.py"),
        "scoped search must include in-scope save candidate: {search_result}"
    );
    assert!(
        !search_result.contains("sandbox/controllers/session_store.py"),
        "scoped search must exclude out-of-scope save candidate: {search_result}"
    );

    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are saved in sandbox/services/session_store.py.")
    );
}

#[test]
fn save_lookup_read_cap_still_applies() {
    // MaxReadsPerTurn must still apply under SaveLookup.
    // The save file is dispatched after the first non-save read; evidence_ready
    // fires once the save file is read, bounding further reads via answer-phase.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    for dir in &["a", "b", "c", "d"] {
        fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    fs::write(
        tmp.path().join("a").join("session.py"),
        "def session_a(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("b").join("session.py"),
        "def session_b(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("c").join("session.py"),
        "def session_c(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("d").join("session.py"),
        "save_session(session)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            // Model reads a non-save file; runtime dispatches the save file, which
            // triggers evidence_ready and bounds remaining reads via answer-phase.
            "[read_file: a/session.py]",
            "[read_file: b/session.py]",
            "[read_file: c/session.py]",
            "[read_file: d/session.py]",
            "Sessions are saved in d/session.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions saved?".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "must not fail (cap is a correction): {events:?}"
    );
    let snapshot = rt.messages_snapshot();
    let read_count = snapshot
        .iter()
        .filter(|m| m.content.contains("=== tool_result: read_file ==="))
        .count();
    assert!(
        read_count <= 3,
        "reads must be bounded to at most 3 per turn; got {read_count}"
    );
}

// Phase 9.2.3 — regression tests for earlier modes/invariants

#[test]
fn create_lookup_read_cap_still_applies() {
    // MaxReadsPerTurn must still apply under CreateLookup.
    // The create file is dispatched after the first non-create read; evidence_ready
    // fires once the create file is read, bounding further reads via answer-phase.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    for dir in &["a", "b", "c", "d"] {
        fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    fs::write(
        tmp.path().join("a").join("task.py"),
        "def task_a():\n    pass\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("b").join("task.py"),
        "def task_b():\n    pass\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("c").join("task.py"),
        "def task_c():\n    pass\n",
    )
    .unwrap();
    fs::write(tmp.path().join("d").join("task.py"), "db.create(task)\n").unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: task]",
            // Model reads a non-create file; runtime dispatches the create file, which
            // triggers evidence_ready and bounds remaining reads via answer-phase.
            "[read_file: a/task.py]",
            "[read_file: b/task.py]",
            "[read_file: c/task.py]",
            "[read_file: d/task.py]",
            "Tasks are created in d/task.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are tasks created?".into(),
        },
    );

    assert!(
        !has_failed(&events),
        "must not fail (cap is a correction): {events:?}"
    );
    let snapshot = rt.messages_snapshot();
    let read_count = snapshot
        .iter()
        .filter(|m| m.content.contains("=== tool_result: read_file ==="))
        .count();
    assert!(
        read_count <= 3,
        "reads must be bounded to at most 3 per turn; got {read_count}"
    );
}

#[test]
fn read_file_command_rejects_absolute_path() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::ReadFile {
            path: "/etc/passwd".to_string(),
        },
    );
    let info: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::InfoMessage(m) = e {
                Some(m.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        info.iter().any(|m| m.contains("path must be relative")),
        "expected absolute path error, got: {info:?}"
    );
    assert!(
        rt.anchors.last_read_file().is_none(),
        "anchor must not be updated on rejected path"
    );
}

#[test]
fn read_file_command_rejects_parent_traversal() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::ReadFile {
            path: "src/../../etc/passwd".to_string(),
        },
    );
    let info: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::InfoMessage(m) = e {
                Some(m.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        info.iter().any(|m| m.contains("'..' components")),
        "expected parent traversal error, got: {info:?}"
    );
    assert!(
        rt.anchors.last_read_file().is_none(),
        "anchor must not be updated on rejected path"
    );
}

#[test]
fn search_code_command_rejects_short_query() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(Vec::<String>::new(), tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::SearchCode {
            query: "a".to_string(),
        },
    );
    let info: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::InfoMessage(m) = e {
                Some(m.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        info.iter().any(|m| m.contains("at least 2 characters")),
        "expected short query error, got: {info:?}"
    );
    assert!(
        rt.anchors.last_search_query().is_none(),
        "anchor must not be updated on rejected query"
    );
}

// ── 18.4 → 18.2 answer guard retry on EvidenceReady ─────────────────────

/// Guard fires on an unread search candidate when evidence is already ready.
/// The guard dispatches a read of the unread candidate regardless of evidence
/// state — evidence_ready and cited-but-unread are independent. Model synthesizes
/// correctly after both files are read → ToolAssisted.
#[test]
fn answer_guard_evidence_ready_text_retry_allows_grounded_synthesis() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/a.rs"), "fn run_turns() {}\n").unwrap();
    fs::write(
        tmp.path().join("src/b.rs"),
        "fn run_turns() {} // also a candidate\n",
    )
    .unwrap();

    // Model reads a.rs (evidence ready) then cites the unread candidate b.rs.
    // Guard fires: b.rs is a candidate → runtime dispatches read of b.rs.
    // Model answers correctly citing only a.rs (now both files read) → ToolAssisted.
    let mut rt = make_runtime_in(
        vec![
            "[search_code: run_turns]",
            "[read_file: src/a.rs]",
            "run_turns is in src/b.rs.", // guard detects unread candidate, dispatches read
            "run_turns is in src/a.rs.", // cites a read file, admitted
        ],
        tmp.path(),
    );
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is run_turns located?".into(),
        },
    );

    let source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(s) = e {
            Some(s.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(source, Some(AnswerSource::ToolAssisted { .. })),
        "guard dispatch must allow grounded synthesis: {source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let read_results = snapshot
        .iter()
        .filter(|m| m.content.contains("=== tool_result: read_file ==="))
        .count();
    assert_eq!(
        read_results, 2,
        "guard must dispatch read of unread candidate (both files read): {snapshot:?}"
    );
}

/// Guard fires on a non-candidate path → can_dispatch is false → Phase 18.3 correction
/// fires → clean synthesis is admitted on retry. Verifies Phase 18.3 is fully preserved.
#[test]
fn answer_guard_correction_fires_when_bad_path_is_not_a_search_candidate() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/engine.rs"), "fn run_turns() {}\n").unwrap();
    fs::write(tmp.path().join("src/unrelated.rs"), "fn unrelated() {}\n").unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: run_turns]",
            "[read_file: src/engine.rs]",
            "run_turns is in src/unrelated.rs.",
            "run_turns is in src/engine.rs.",
        ],
        tmp.path(),
    );
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is run_turns located?".into(),
        },
    );

    let source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(s) = e {
            Some(s.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(source, Some(AnswerSource::ToolAssisted { .. })),
        "Phase 18.3 correction must allow clean synthesis on retry: {source:?}"
    );
    let snapshot = rt.messages_snapshot();
    assert!(
        snapshot.iter().any(|m| {
            m.content.contains("[runtime:correction]") && m.content.contains("src/unrelated.rs")
        }),
        "correction must name the cited non-candidate path: {snapshot:?}"
    );
}

/// Guard fires once (dispatch), retry flag blocks a second dispatch on the next
/// violation — terminal fires instead. Verifies no double-dispatch is possible.
#[test]
fn answer_guard_terminal_fires_on_second_violation_after_dispatch() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/a.rs"), "fn run_turns() {}\n").unwrap();
    fs::write(tmp.path().join("src/b.rs"), "fn run_turns() {} // b\n").unwrap();
    fs::write(tmp.path().join("src/c.rs"), "fn run_turns() {} // c\n").unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: run_turns]",
            "[read_file: src/a.rs]",
            "run_turns is in src/b.rs.", // guard fires → dispatch reads b.rs
            "run_turns is in src/c.rs.", // guard fires again → terminal
        ],
        tmp.path(),
    );
    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is run_turns located?".into(),
        },
    );

    let source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(s) = e {
            Some(s.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(
            source,
            Some(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::InsufficientEvidence,
                ..
            })
        ),
        "second guard violation after dispatch must terminate: {source:?}"
    );
}

#[test]
fn undo_with_empty_stack_emits_nothing_to_undo_message() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(&mut rt, RuntimeRequest::Undo);

    let system_messages: Vec<&str> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::SystemMessage(msg) = e {
                Some(msg.as_str())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        system_messages,
        vec!["Nothing to undo."],
        "empty undo stack must emit exactly the nothing-to-undo message"
    );
    assert!(
        !has_failed(&events),
        "undo on empty stack must not emit Failed"
    );
}

#[test]
fn providers_use_unknown_name_emits_error_system_message() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mut rt = make_runtime_in(vec![] as Vec<String>, tmp.path());
    let events = collect_events(
        &mut rt,
        RuntimeRequest::ProvidersUse {
            name: "totally_unknown".to_string(),
        },
    );

    assert!(
        events.iter().any(|e| matches!(
            e,
            RuntimeEvent::SystemMessage(msg) if msg.contains("Unknown provider")
        )),
        "unknown provider name must emit SystemMessage with 'Unknown provider': {events:?}"
    );
    assert!(
        !has_failed(&events),
        "unknown provider must not emit Failed"
    );
}

#[test]
fn non_cargo_verify_passes_raw_output_unchanged() {
    // A non-cargo verify command that produces non-empty output must pass the raw text
    // through to the correction message without JSON parsing.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let data_file = tmp.path().join("data.txt");
    fs::write(&data_file, "hello world\n").unwrap();

    let abs_path = data_file.to_string_lossy().into_owned();
    let payload = format!("{abs_path}\x00hello world\x00hello there");

    // "echo failure-marker" exits 0 but produces non-empty output — the non-cargo path
    // treats any non-empty output as a failure requiring correction.
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path())
        .with_verify_command(Some("echo failure-marker".into()))
        .with_deferred_verify(false)
        .with_max_correction_attempts(0);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not panic: {events:?}");
    let has_raw_output = events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("failure-marker")));
    assert!(
        has_raw_output,
        "non-cargo verify raw output must appear in correction SystemMessage: {events:?}"
    );
}

#[test]
fn verify_returning_none_does_not_inject_correction() {
    // When run_verify_command returns None (empty output), no correction must be
    // injected — confirmed by the absence of any "manual fix required" SystemMessage.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let data_file = tmp.path().join("data.txt");
    fs::write(&data_file, "hello world\n").unwrap();

    let abs_path = data_file.to_string_lossy().into_owned();
    let payload = format!("{abs_path}\x00hello world\x00hello there");

    // "true" always exits 0 with no output — run_verify_command must return None.
    let mut rt = make_runtime_in(Vec::<&str>::new(), tmp.path())
        .with_verify_command(Some("true".into()))
        .with_max_correction_attempts(0);
    rt.set_pending_for_test(PendingAction {
        tool_name: "edit_file".into(),
        summary: format!("edit {abs_path}"),
        risk: RiskLevel::Low,
        payload,
    });

    let events = collect_events(&mut rt, RuntimeRequest::Approve);
    assert!(!has_failed(&events), "approve must not panic: {events:?}");
    let has_correction = events.iter().any(
        |e| matches!(e, RuntimeEvent::SystemMessage(msg) if msg.contains("manual fix required")),
    );
    assert!(
        !has_correction,
        "verify returning None must not inject correction: {events:?}"
    );
}
