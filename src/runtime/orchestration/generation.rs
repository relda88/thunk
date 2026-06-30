use crate::core::error::Result;
use crate::llm::backend::{
    BackendEvent, BackendStatus, ConstrainedMode, GenerateRequest, Message, ModelBackend,
};

use super::super::conversation::Conversation;
use super::super::investigation::investigation::InvestigationMode;
use super::super::investigation::tool_surface::ToolSurface;
use super::super::protocol::prompt;
use super::super::protocol::prompt_physics::{self, PromptPhysicsConfig};
use super::super::protocol::response_text::strip_thunk_context_block;
use super::super::trace::trace_runtime_decision;
use super::super::types::{Activity, RuntimeEvent};

/// Runs a single generation turn: sends the current conversation to the backend,
/// buffers the assistant response into conversation history, then returns the
/// complete response text, or None if the backend produced no output. Assistant
/// message events are emitted only after runtime admission.
#[allow(clippy::too_many_arguments)] // orchestration function wiring backend, conversation, and policy
pub(super) fn run_generate_turn(
    backend: &mut dyn ModelBackend,
    conversation: &mut Conversation,
    tool_surface: ToolSurface,
    project_snapshot_hint: Option<&str>,
    test_coverage_hint: Option<&str>,
    investigation_mode: InvestigationMode,
    prompt_physics: &PromptPhysicsConfig,
    constrained_output: bool,
    recall_facts: &[crate::storage::memory::MemoryFact],
    dynamic_tool_names: &[&str],
    on_event: &mut dyn FnMut(RuntimeEvent),
) -> Result<Option<String>> {
    let mut messages = conversation.pruned_snapshot();
    // Primacy anchor: ability + thunk_md injected at position 1
    // (after stored system prompt, before conversation history)
    let has_primacy = if let Some(primacy) = prompt_physics::primacy_anchor_block(prompt_physics) {
        messages.insert(1, Message::system(primacy));
        true
    } else {
        false
    };
    let mut hint_tools: Vec<&str> = tool_surface
        .allowed_tool_names()
        .chain(tool_surface.mutation_tool_names().iter().copied())
        .collect();
    hint_tools.extend_from_slice(dynamic_tool_names);
    messages.push(Message::system(prompt::render_tool_surface_hint(
        tool_surface.as_str(),
        hint_tools,
    )));
    if let Some(hint) = project_snapshot_hint {
        messages.push(Message::system(hint.to_string()));
    }
    if let Some(hint) = test_coverage_hint {
        messages.push(Message::system(hint.to_string()));
    }
    let has_refresh =
        if let Some(refresh) = prompt_physics::periodic_refresh_message(prompt_physics) {
            messages.push(Message::system(refresh));
            true
        } else {
            false
        };
    // Suppress recency injection on AnswerOnly: this surface covers both pure synthesis turns
    // and PostRead answer phase (engine maps answer_phase.is_some() → AnswerOnly).
    // AnswerOnly has no tools and no investigation context to update; the [thunk: current context]
    // block only adds noise and can leak markers into the admitted answer text.
    let has_recency = if !matches!(tool_surface, ToolSurface::AnswerOnly) {
        if let Some(recency) =
            prompt_physics::recency_field_message(prompt_physics, tool_surface, dynamic_tool_names)
        {
            messages.push(Message::system(recency));
            true
        } else {
            false
        }
    } else {
        false
    };
    if !recall_facts.is_empty() {
        let recall_block = format!(
            "## Relevant context from memory\n{}",
            recall_facts
                .iter()
                .map(|f| format!("- {}\n", f.text))
                .collect::<String>()
        );
        messages.push(Message::system(recall_block));
    }
    {
        let mut components = Vec::new();
        if has_primacy {
            components.push("primacy");
        }
        if has_refresh {
            components.push("refresh");
        }
        if has_recency {
            components.push("recency");
        }
        if has_primacy
            && prompt_physics.compress_abilities
            && prompt_physics.active_ability.is_some()
        {
            components.push("compression");
        }
        if !components.is_empty() {
            trace_runtime_decision(
                on_event,
                "prompt_physics_injected",
                &[("components", components.join(","))],
            );
        }
    }
    let request = GenerateRequest {
        messages,
        constrained_mode: match (constrained_output, tool_surface) {
            (true, ToolSurface::MutationEnabled) => ConstrainedMode::Edit,
            (true, _) => ConstrainedMode::ToolCall,
            _ => ConstrainedMode::None,
        },
    };
    let mut response = String::new();

    let result = backend.generate(request, &mut |event| match event {
        BackendEvent::StatusChanged(status) => {
            on_event(RuntimeEvent::ActivityChanged(map_backend_status(
                status,
                investigation_mode,
            )));
        }
        BackendEvent::TextDelta(chunk) => {
            response.push_str(&chunk);
        }
        BackendEvent::Timing { stage, elapsed_ms } => {
            on_event(RuntimeEvent::BackendTiming { stage, elapsed_ms });
        }
        BackendEvent::TokenCounts { prompt, completion } => {
            on_event(RuntimeEvent::BackendTokenCounts { prompt, completion });
        }
        BackendEvent::PromptAssembled(p) => {
            on_event(RuntimeEvent::PromptAssembled(p));
        }
        BackendEvent::Finished => {}
    });

    result?;

    if response.is_empty() {
        Ok(None)
    } else {
        conversation.begin_assistant_reply();
        conversation.push_assistant_chunk(&response);
        Ok(Some(response))
    }
}

pub(super) fn emit_visible_assistant_message(text: &str, on_event: &mut dyn FnMut(RuntimeEvent)) {
    on_event(RuntimeEvent::ActivityChanged(Activity::Responding));
    on_event(RuntimeEvent::AssistantMessageStarted);
    let clean = strip_thunk_context_block(text);
    on_event(RuntimeEvent::AssistantMessageChunk(clean.into_owned()));
    on_event(RuntimeEvent::AssistantMessageFinished);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::llm::backend::{
        BackendCapabilities, BackendEvent, GenerateRequest, Message, ModelBackend, Role,
    };
    use crate::runtime::conversation::Conversation;
    use crate::runtime::investigation::investigation::InvestigationMode;
    use crate::runtime::investigation::tool_surface::ToolSurface;
    use crate::runtime::protocol::prompt_physics::PromptPhysicsConfig;
    use crate::storage::memory::{MemoryFact, MemorySource};

    struct CapturingBackend {
        captured: Arc<Mutex<Vec<GenerateRequest>>>,
    }

    impl ModelBackend for CapturingBackend {
        fn name(&self) -> &str {
            "capture"
        }
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities {
                context_window_tokens: None,
                max_output_tokens: None,
            }
        }
        fn generate(
            &mut self,
            req: GenerateRequest,
            on_event: &mut dyn FnMut(BackendEvent),
        ) -> crate::core::error::Result<()> {
            self.captured.lock().unwrap().push(req);
            on_event(BackendEvent::TextDelta("ok".to_string()));
            on_event(BackendEvent::Finished);
            Ok(())
        }
    }

    fn make_fact(text: &str) -> MemoryFact {
        MemoryFact {
            id: 1,
            text: text.to_string(),
            category: "test".to_string(),
            scope: None,
            salience: 1.0,
            embedding: None,
            model_name: None,
            source: MemorySource::User,
            created_at: "2026-01-01".to_string(),
            updated_at: "2026-01-01".to_string(),
            last_recalled_at: None,
        }
    }

    #[test]
    fn recall_facts_are_request_local() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut backend = CapturingBackend {
            captured: Arc::clone(&captured),
        };
        let mut conversation = Conversation::new("system".to_string());
        conversation.push_user("hello");

        let fact = make_fact("user prefers dark mode");
        let recall_facts = [fact];

        let physics = PromptPhysicsConfig::default();
        super::run_generate_turn(
            &mut backend,
            &mut conversation,
            ToolSurface::RetrievalFirst,
            None,
            None,
            InvestigationMode::General,
            &physics,
            false,
            &recall_facts,
            &[],
            &mut |_| {},
        )
        .unwrap();

        let reqs = captured.lock().unwrap();
        let req = reqs.first().expect("backend must have received a request");

        // Recall block must appear in the backend request messages
        assert!(
            req.messages.iter().any(|m| {
                m.role == Role::System && m.content.contains("user prefers dark mode")
            }),
            "recall fact must appear in backend request: {:?}",
            req.messages
        );

        // Recall block must NOT appear in conversation history
        let conv_messages: Vec<Message> = conversation.pruned_snapshot();
        assert!(
            !conv_messages
                .iter()
                .any(|m| m.content.contains("user prefers dark mode")),
            "recall fact must not persist in conversation history: {:?}",
            conv_messages
        );
    }
}

fn map_backend_status(status: BackendStatus, investigation_mode: InvestigationMode) -> Activity {
    match status {
        BackendStatus::LoadingModel => Activity::LoadingModel,
        BackendStatus::CreatingContext => Activity::CreatingContext,
        BackendStatus::Tokenizing => Activity::Tokenizing,
        BackendStatus::Prefilling => Activity::Prefilling,
        BackendStatus::Generating => Activity::Generating {
            mode: Some(match investigation_mode {
                InvestigationMode::General => "Synthesizing answer".to_string(),
                _ => "Investigating".to_string(),
            }),
        },
    }
}
