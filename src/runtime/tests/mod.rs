use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::core::config::Config;
use crate::llm::backend::{BackendCapabilities, BackendEvent, GenerateRequest, ModelBackend};
use crate::tools::default_registry;

pub use super::{
    AnswerSource, PendingAction, ProjectRoot, RiskLevel, Runtime, RuntimeEvent, RuntimeRequest,
};

mod ability_skill;
mod agent_command;
mod anchors;
mod approval;
mod branch_commands;
mod commit_command;
mod context_threshold;
mod diff_command;
mod engine;
mod external_repo_fixtures;
mod finalization;
mod git_acquisition;
mod index_embed;
mod integration;
mod integration_misc;
mod investigation;
mod investigation_inline;
mod investigation_modes;
mod path_scope;
mod plan_command;
mod project_snapshot;
mod prompt_physics;
mod read_bounds;
mod search_budget;
mod search_guardrails;
mod task_command;
mod tool_round;
mod tool_surface;
mod web_fetch;

pub struct TestBackend {
    responses: Vec<String>,
    call_count: usize,
}

impl TestBackend {
    pub fn new(responses: Vec<impl Into<String>>) -> Self {
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

pub struct RecordingBackend {
    responses: Vec<String>,
    call_count: usize,
    requests: Arc<Mutex<Vec<GenerateRequest>>>,
}

impl RecordingBackend {
    pub fn new(
        responses: Vec<impl Into<String>>,
        requests: Arc<Mutex<Vec<GenerateRequest>>>,
    ) -> Self {
        Self {
            responses: responses.into_iter().map(Into::into).collect(),
            call_count: 0,
            requests,
        }
    }
}

impl ModelBackend for RecordingBackend {
    fn name(&self) -> &str {
        "recording-test"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            context_window_tokens: None,
            max_output_tokens: None,
        }
    }

    fn generate(
        &mut self,
        request: GenerateRequest,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> crate::core::error::Result<()> {
        self.requests.lock().unwrap().push(request);
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

pub fn make_runtime(responses: Vec<impl Into<String>>) -> Runtime {
    let root = ProjectRoot::new(PathBuf::from(".")).unwrap();
    Runtime::new(
        &Config::default(),
        root.clone(),
        Box::new(TestBackend::new(responses)),
        default_registry().with_project_root(root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-session".to_string(),
    )
}

pub fn make_runtime_in(responses: Vec<impl Into<String>>, root: &std::path::Path) -> Runtime {
    let project_root = ProjectRoot::new(root.to_path_buf()).unwrap();
    Runtime::new(
        &Config::default(),
        project_root.clone(),
        Box::new(TestBackend::new(responses)),
        default_registry().with_project_root(project_root.as_path_buf()),
        None,
        root.to_path_buf(),
        "test-session".to_string(),
    )
}

pub fn make_runtime_with_recorded_requests(
    responses: Vec<impl Into<String>>,
) -> (Runtime, Arc<Mutex<Vec<GenerateRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let root = ProjectRoot::new(PathBuf::from(".")).unwrap();
    let runtime = Runtime::new(
        &Config::default(),
        root.clone(),
        Box::new(RecordingBackend::new(responses, Arc::clone(&requests))),
        default_registry().with_project_root(root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-session".to_string(),
    );
    (runtime, requests)
}

pub fn collect_events(runtime: &mut Runtime, request: RuntimeRequest) -> Vec<RuntimeEvent> {
    let mut events = Vec::new();
    runtime.handle(request, &mut |e| events.push(e));
    events
}

/// Backend that emits a configurable token count alongside its response so that
/// `TurnPerformance.tokens_prompt` is populated without requiring THUNK_TRACE_RUNTIME.
pub struct TokenCountingBackend {
    responses: Vec<String>,
    call_count: usize,
    /// Reported as `BackendEvent::TokenCounts { prompt, .. }` on each generate call.
    prompt_tokens_per_call: u32,
    context_window_tokens: Option<u32>,
}

impl TokenCountingBackend {
    pub fn new(
        responses: Vec<impl Into<String>>,
        prompt_tokens_per_call: u32,
        context_window_tokens: Option<u32>,
    ) -> Self {
        Self {
            responses: responses.into_iter().map(Into::into).collect(),
            call_count: 0,
            prompt_tokens_per_call,
            context_window_tokens,
        }
    }
}

impl ModelBackend for TokenCountingBackend {
    fn name(&self) -> &str {
        "token-counting-test"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            context_window_tokens: self.context_window_tokens,
            max_output_tokens: None,
        }
    }

    fn generate(
        &mut self,
        _request: GenerateRequest,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> crate::core::error::Result<()> {
        on_event(BackendEvent::TokenCounts {
            prompt: self.prompt_tokens_per_call,
            completion: 0,
        });
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

pub fn make_runtime_with_token_counting_backend(
    responses: Vec<impl Into<String>>,
    prompt_tokens_per_call: u32,
    context_window_tokens: Option<u32>,
) -> Runtime {
    let root = ProjectRoot::new(PathBuf::from(".")).unwrap();
    Runtime::new(
        &Config::default(),
        root.clone(),
        Box::new(TokenCountingBackend::new(
            responses,
            prompt_tokens_per_call,
            context_window_tokens,
        )),
        default_registry().with_project_root(root.as_path_buf()),
        None,
        PathBuf::from("/tmp"),
        "test-session".to_string(),
    )
}

pub fn init_git_repo(root: &std::path::Path) {
    let status = std::process::Command::new("git")
        .args(["init"])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git init must succeed");
}

pub fn has_failed(events: &[RuntimeEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::Failed { .. }))
}

pub fn failed_message(events: &[RuntimeEvent]) -> Option<String> {
    events.iter().find_map(|e| {
        if let RuntimeEvent::Failed { message } = e {
            Some(message.clone())
        } else {
            None
        }
    })
}
