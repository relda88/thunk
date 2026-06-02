use crate::core::error::Result;
use crate::llm::backend::{BackendEvent, BackendStatus, GenerateRequest, Message, ModelBackend};

use super::super::conversation::Conversation;
use super::super::investigation::investigation::InvestigationMode;
use super::super::investigation::tool_surface::ToolSurface;
use super::super::protocol::prompt;
use super::super::protocol::prompt_physics::{self, PromptPhysicsConfig};
use super::super::types::{Activity, RuntimeEvent};

/// Runs a single generation turn: sends the current conversation to the backend,
/// buffers the assistant response into conversation history, then returns the
/// complete response text, or None if the backend produced no output. Assistant
/// message events are emitted only after runtime admission.
pub(super) fn run_generate_turn(
    backend: &mut dyn ModelBackend,
    conversation: &mut Conversation,
    tool_surface: ToolSurface,
    project_snapshot_hint: Option<&str>,
    investigation_mode: InvestigationMode,
    prompt_physics: &PromptPhysicsConfig,
    on_event: &mut dyn FnMut(RuntimeEvent),
) -> Result<Option<String>> {
    let mut messages = conversation.pruned_snapshot();
    messages.push(Message::system(prompt::render_tool_surface_hint(
        tool_surface.as_str(),
        tool_surface
            .allowed_tool_names()
            .chain(tool_surface.mutation_tool_names().iter().copied()),
    )));
    if let Some(hint) = project_snapshot_hint {
        messages.push(Message::system(hint.to_string()));
    }
    if let Some(refresh) = prompt_physics::periodic_refresh_message(prompt_physics) {
        messages.push(Message::system(refresh));
    }
    if let Some(recency) = prompt_physics::recency_field_message(prompt_physics, tool_surface) {
        messages.push(Message::system(recency));
    }
    let request = GenerateRequest::new(messages);
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
    on_event(RuntimeEvent::AssistantMessageChunk(text.to_string()));
    on_event(RuntimeEvent::AssistantMessageFinished);
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
