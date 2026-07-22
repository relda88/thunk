use serde::Serialize;

use crate::runtime::{Activity, AnswerSource, RuntimeEvent, RuntimeTerminalReason};
use crate::storage::session::SessionMeta;
use crate::tools::{PendingAction, RiskLevel};
use crate::tui::decode_approval_preview;
use crate::tui::worker::WorkerReply;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevelDto {
    Low,
    Medium,
    High,
}

impl From<RiskLevel> for RiskLevelDto {
    fn from(r: RiskLevel) -> Self {
        match r {
            RiskLevel::Low => Self::Low,
            RiskLevel::Medium => Self::Medium,
            RiskLevel::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingActionDto {
    pub tool_name: String,
    pub summary: String,
    pub risk: RiskLevelDto,
    pub reversible: bool,
    pub preview: Vec<String>,
}

impl From<PendingAction> for PendingActionDto {
    fn from(a: PendingAction) -> Self {
        let preview = decode_approval_preview(&a.tool_name, &a.payload);
        Self {
            tool_name: a.tool_name,
            summary: a.summary,
            risk: a.risk.into(),
            reversible: a.reversible,
            preview,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityDto {
    Idle,
    Processing,
    LoadingModel,
    CreatingContext,
    Tokenizing,
    Prefilling,
    Generating {
        mode: Option<String>,
    },
    Responding,
    ExecutingTools {
        tool: String,
        detail: Option<String>,
    },
    AwaitingApproval {
        tool: String,
    },
}

impl From<Activity> for ActivityDto {
    fn from(a: Activity) -> Self {
        match a {
            Activity::Idle => Self::Idle,
            Activity::Processing => Self::Processing,
            Activity::LoadingModel => Self::LoadingModel,
            Activity::CreatingContext => Self::CreatingContext,
            Activity::Tokenizing => Self::Tokenizing,
            Activity::Prefilling => Self::Prefilling,
            Activity::Generating { mode } => Self::Generating { mode },
            Activity::Responding => Self::Responding,
            Activity::ExecutingTools { tool, detail } => Self::ExecutingTools { tool, detail },
            Activity::AwaitingApproval { tool } => Self::AwaitingApproval { tool },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTerminalReasonDto {
    RejectedMutation,
    ReadFileFailed,
    MutationFailed,
    RepeatedDisallowedTool,
    RepeatedSearchBudgetViolation,
    RepeatedFabricatedToolResult,
    RepeatedMalformedToolSyntax,
    RepeatedGarbledEditRepair,
    RepeatedToolAfterEvidenceReady,
    RepeatedWeakSearchQuery,
    RepeatedToolAfterAnswerPhase,
    InsufficientEvidence,
    ExecDisabled,
    McpInProjectRedirect,
}

impl From<RuntimeTerminalReason> for RuntimeTerminalReasonDto {
    fn from(r: RuntimeTerminalReason) -> Self {
        match r {
            RuntimeTerminalReason::RejectedMutation => Self::RejectedMutation,
            RuntimeTerminalReason::ReadFileFailed => Self::ReadFileFailed,
            RuntimeTerminalReason::MutationFailed => Self::MutationFailed,
            RuntimeTerminalReason::RepeatedDisallowedTool => Self::RepeatedDisallowedTool,
            RuntimeTerminalReason::RepeatedSearchBudgetViolation => {
                Self::RepeatedSearchBudgetViolation
            }
            RuntimeTerminalReason::RepeatedFabricatedToolResult => {
                Self::RepeatedFabricatedToolResult
            }
            RuntimeTerminalReason::RepeatedMalformedToolSyntax => Self::RepeatedMalformedToolSyntax,
            RuntimeTerminalReason::RepeatedGarbledEditRepair => Self::RepeatedGarbledEditRepair,
            RuntimeTerminalReason::RepeatedToolAfterEvidenceReady => {
                Self::RepeatedToolAfterEvidenceReady
            }
            RuntimeTerminalReason::RepeatedWeakSearchQuery => Self::RepeatedWeakSearchQuery,
            RuntimeTerminalReason::RepeatedToolAfterAnswerPhase => {
                Self::RepeatedToolAfterAnswerPhase
            }
            RuntimeTerminalReason::InsufficientEvidence => Self::InsufficientEvidence,
            RuntimeTerminalReason::ExecDisabled => Self::ExecDisabled,
            RuntimeTerminalReason::McpInProjectRedirect => Self::McpInProjectRedirect,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnswerSourceDto {
    Direct,
    ToolAssisted {
        rounds: usize,
    },
    RuntimeTerminal {
        reason: RuntimeTerminalReasonDto,
        rounds: usize,
    },
    ToolLimitReached,
}

impl From<AnswerSource> for AnswerSourceDto {
    fn from(s: AnswerSource) -> Self {
        match s {
            AnswerSource::Direct => Self::Direct,
            AnswerSource::ToolAssisted { rounds } => Self::ToolAssisted { rounds },
            AnswerSource::RuntimeTerminal { reason, rounds } => Self::RuntimeTerminal {
                reason: reason.into(),
                rounds,
            },
            AnswerSource::ToolLimitReached => Self::ToolLimitReached,
        }
    }
}

/// Serializable mirror of `RuntimeEvent` for the GUI frontend.
///
/// Advisory variants (`BackendTiming`, `BackendTokenCounts`, `RuntimeTrace`,
/// `PromptAssembled`) are intentionally absent — `TryFrom` returns `Err(())`
/// for them so they never reach the webview.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEventDto {
    ActivityChanged {
        activity: ActivityDto,
    },
    AssistantMessageStarted,
    AssistantMessageChunk {
        chunk: String,
    },
    AssistantMessageFinished,
    ToolCallStarted {
        name: String,
    },
    ToolCallFinished {
        name: String,
        summary: Option<String>,
    },
    ApprovalRequired {
        pending: PendingActionDto,
        evidence: Vec<String>,
        impact: Vec<String>,
        reason: Option<String>,
    },
    TransactionApprovalRequired {
        actions: Vec<PendingActionDto>,
        evidence: Vec<String>,
        impact: Vec<String>,
        reason: Option<String>,
    },
    AnswerReady {
        source: AnswerSourceDto,
    },
    Failed {
        message: String,
    },
    InfoMessage {
        text: String,
    },
    SystemMessage {
        text: String,
    },
    FileReadFinished {
        path: String,
        line_count: usize,
        content: String,
    },
    DirectReadCompleted,
    ContextUsage {
        prompt_tokens: u64,
        context_window_tokens: u32,
    },
    PlanApprovalRequired {
        goal: String,
        steps: Vec<(String, String)>,
    },
    PlanApprovalCleared,
    MemoryProposalRequired {
        fact: String,
        category: String,
        scope: Option<String>,
        source: String,
        delete: bool,
    },
    MemoryProposalCleared,
    ResetOk,
}

impl TryFrom<RuntimeEvent> for RuntimeEventDto {
    type Error = ();

    fn try_from(event: RuntimeEvent) -> Result<Self, ()> {
        let dto = match event {
            RuntimeEvent::ActivityChanged(a) => Self::ActivityChanged { activity: a.into() },
            RuntimeEvent::AssistantMessageStarted => Self::AssistantMessageStarted,
            RuntimeEvent::AssistantMessageChunk(chunk) => Self::AssistantMessageChunk { chunk },
            RuntimeEvent::AssistantMessageFinished => Self::AssistantMessageFinished,
            RuntimeEvent::ToolCallStarted { name } => Self::ToolCallStarted { name },
            RuntimeEvent::ToolCallFinished { name, summary } => {
                Self::ToolCallFinished { name, summary }
            }
            RuntimeEvent::ApprovalRequired {
                pending,
                evidence,
                impact,
                reason,
            } => Self::ApprovalRequired {
                pending: pending.into(),
                evidence,
                impact,
                reason,
            },
            RuntimeEvent::TransactionApprovalRequired {
                actions,
                evidence,
                impact,
                reason,
            } => Self::TransactionApprovalRequired {
                actions: actions.into_iter().map(Into::into).collect(),
                evidence,
                impact,
                reason,
            },
            RuntimeEvent::AnswerReady(source) => Self::AnswerReady {
                source: source.into(),
            },
            RuntimeEvent::Failed { message } => Self::Failed { message },
            RuntimeEvent::InfoMessage(text) => Self::InfoMessage { text },
            RuntimeEvent::SystemMessage(text) => Self::SystemMessage { text },
            RuntimeEvent::FileReadFinished {
                path,
                line_count,
                content,
            } => Self::FileReadFinished {
                path,
                line_count,
                content,
            },
            RuntimeEvent::DirectReadCompleted => Self::DirectReadCompleted,
            RuntimeEvent::ContextUsage {
                prompt_tokens,
                context_window_tokens,
            } => Self::ContextUsage {
                prompt_tokens,
                context_window_tokens,
            },
            RuntimeEvent::PlanApprovalRequired { goal, steps } => {
                Self::PlanApprovalRequired { goal, steps }
            }
            RuntimeEvent::PlanApprovalCleared => Self::PlanApprovalCleared,
            RuntimeEvent::MemoryProposalRequired {
                fact,
                category,
                scope,
                source,
                delete,
            } => Self::MemoryProposalRequired {
                fact,
                category,
                scope,
                source,
                delete,
            },
            RuntimeEvent::MemoryProposalCleared => Self::MemoryProposalCleared,
            // Advisory variants — dropped here, never forwarded to the GUI.
            RuntimeEvent::BackendTiming { .. }
            | RuntimeEvent::BackendTokenCounts { .. }
            | RuntimeEvent::RuntimeTrace(_)
            | RuntimeEvent::PromptAssembled(_) => return Err(()),
        };
        Ok(dto)
    }
}

/// Convert a `WorkerReply` to an optional GUI event DTO.
///
/// Returns `None` for replies with no visual representation in the GUI
/// (e.g. `HandleOk`). Returns `Some` for anything the frontend
/// should render or react to.
pub fn worker_reply_to_dto(reply: WorkerReply) -> Option<RuntimeEventDto> {
    match reply {
        WorkerReply::Event(ev) => RuntimeEventDto::try_from(ev).ok(),
        WorkerReply::DeferredVerification(msg) => {
            Some(RuntimeEventDto::SystemMessage { text: msg })
        }
        WorkerReply::HandleOk => None,
        WorkerReply::HandleErr(msg) => Some(RuntimeEventDto::Failed { message: msg }),
        WorkerReply::ResetOk => Some(RuntimeEventDto::ResetOk),
        WorkerReply::ResetErr(msg) => Some(RuntimeEventDto::Failed { message: msg }),
        WorkerReply::SessionsOk(sessions) => Some(RuntimeEventDto::SystemMessage {
            text: format_sessions_list(&sessions),
        }),
        WorkerReply::SessionsErr(msg) => Some(RuntimeEventDto::Failed { message: msg }),
        WorkerReply::ClearOk => Some(RuntimeEventDto::SystemMessage {
            text: "Project sessions cleared.".to_string(),
        }),
        WorkerReply::ClearErr(msg) => Some(RuntimeEventDto::Failed { message: msg }),
    }
}

fn format_sessions_list(sessions: &[SessionMeta]) -> String {
    if sessions.is_empty() {
        return "current project sessions: none".to_string();
    }
    let mut lines = vec!["current project sessions:".to_string()];
    for session in sessions {
        lines.push(format!(
            "{}  |  {}  |  {} messages",
            session.id,
            format_session_timestamp(session.updated_at),
            session.message_count
        ));
    }
    lines.join("\n")
}

fn format_session_timestamp(ts: u64) -> String {
    let seconds: i64 = if ts >= 1_000_000_000_000_000 {
        (ts / 1_000_000_000) as i64
    } else if ts >= 10_000_000_000 {
        (ts / 1_000) as i64
    } else {
        ts as i64
    };
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn civil_from_unix_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::backend::BackendTimingStage;
    use crate::runtime::RuntimeEvent;

    #[test]
    fn backend_timing_is_not_a_dto() {
        let ev = RuntimeEvent::BackendTiming {
            stage: BackendTimingStage::ModelLoad,
            elapsed_ms: 5,
        };
        assert!(RuntimeEventDto::try_from(ev).is_err());
    }

    #[test]
    fn backend_token_counts_is_not_a_dto() {
        let ev = RuntimeEvent::BackendTokenCounts {
            prompt: 100,
            completion: 50,
        };
        assert!(RuntimeEventDto::try_from(ev).is_err());
    }

    #[test]
    fn runtime_trace_is_not_a_dto() {
        let ev = RuntimeEvent::RuntimeTrace("some trace".into());
        assert!(RuntimeEventDto::try_from(ev).is_err());
    }

    #[test]
    fn prompt_assembled_is_not_a_dto() {
        let ev = RuntimeEvent::PromptAssembled("prompt text".into());
        assert!(RuntimeEventDto::try_from(ev).is_err());
    }

    #[test]
    fn system_message_converts_and_serializes() {
        let ev = RuntimeEvent::SystemMessage("hello".into());
        let dto = RuntimeEventDto::try_from(ev).unwrap();
        let json = serde_json::to_string(&dto).unwrap();
        assert_eq!(json, r#"{"type":"system_message","text":"hello"}"#);
    }

    #[test]
    fn reset_ok_maps_to_reset_ok_dto_and_serializes() {
        let dto = worker_reply_to_dto(WorkerReply::ResetOk).expect("ResetOk must produce a DTO");
        let json = serde_json::to_string(&dto).unwrap();
        assert_eq!(json, r#"{"type":"reset_ok"}"#);
    }

    #[test]
    fn reset_err_still_maps_to_failed() {
        let dto = worker_reply_to_dto(WorkerReply::ResetErr("boom".into()))
            .expect("ResetErr must produce a DTO");
        let json = serde_json::to_string(&dto).unwrap();
        assert_eq!(json, r#"{"type":"failed","message":"boom"}"#);
    }

    #[test]
    fn memory_proposal_required_preserves_all_fields() {
        let ev = RuntimeEvent::MemoryProposalRequired {
            fact: "User prefers Rust".into(),
            category: "preference".into(),
            scope: Some("coding".into()),
            source: "imperative".into(),
            delete: false,
        };
        let dto = RuntimeEventDto::try_from(ev).unwrap();
        match dto {
            RuntimeEventDto::MemoryProposalRequired {
                fact,
                category,
                scope,
                source,
                delete,
            } => {
                assert_eq!(fact, "User prefers Rust");
                assert_eq!(category, "preference");
                assert_eq!(scope.as_deref(), Some("coding"));
                assert_eq!(source, "imperative");
                assert!(!delete);
            }
            _ => panic!("wrong variant"),
        }
    }
}
