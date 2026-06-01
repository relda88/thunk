use crate::runtime::{AnswerSource, RuntimeEvent};
use crate::tools::RiskLevel;

use super::format::summarize_command_output;
use super::state::{AppState, ApprovalRisk, DirtySections, PendingApprovalState};

pub(super) fn decode_approval_preview(tool_name: &str, payload: &str) -> Vec<String> {
    match tool_name {
        "edit_file" => {
            let parts: Vec<&str> = payload.splitn(5, '\x00').collect();
            if parts.len() < 5 {
                return vec![];
            }
            let search_lines = parts[3].lines().map(|l| format!("- {l}"));
            let replace_lines = parts[4].lines().map(|l| format!("+ {l}"));
            search_lines.chain(replace_lines).take(4).collect()
        }
        "shell" => {
            if payload.is_empty() {
                vec![]
            } else {
                vec![payload.to_string()]
            }
        }
        "write_file" => {
            let parts: Vec<&str> = payload.splitn(4, '\x00').collect();
            if parts.len() < 4 {
                return vec![];
            }
            parts[3].lines().take(3).map(|l| format!("  {l}")).collect()
        }
        _ => vec![],
    }
}

pub(super) fn apply_runtime_event(state: &mut AppState, event: RuntimeEvent) {
    match event {
        RuntimeEvent::ActivityChanged(activity) => state.set_status(&activity.label()),
        RuntimeEvent::AssistantMessageStarted => state.begin_assistant_message(),
        RuntimeEvent::AssistantMessageChunk(chunk) => state.append_assistant_chunk(&chunk),
        RuntimeEvent::AssistantMessageFinished => {}
        RuntimeEvent::ToolCallStarted { name } => {
            state.add_collapsible_tool_message(format!("tool: {name}"));
        }
        RuntimeEvent::ToolCallFinished { name, summary } => match summary {
            // FileReadFinished fires for every successful read_file and adds the
            // canonical "read {path} ({n} lines) — Ctrl+O to expand" message.
            // Suppress the compact ToolCallFinished duplicate to keep a single summary.
            Some(_) if name == "read_file" => {}
            Some(s) => state.add_collapsible_tool_message(s),
            None => state.add_tool_message(format!("tool failed: {name}")),
        },
        RuntimeEvent::AnswerReady(source) => {
            state.is_busy = false;
            state.pending_approval = None;
            state.mark_dirty(DirtySections::INPUT);
            state.set_status("ready");
            if let AnswerSource::ToolLimitReached = source {
                state.add_system_message("Tool limit reached. Response may be incomplete.");
            }
        }
        RuntimeEvent::Failed { message } => {
            state.is_busy = false;
            state.pending_approval = None;
            state.mark_dirty(DirtySections::INPUT);
            state.set_status("error");
            state.add_error_message(message);
        }
        RuntimeEvent::ApprovalRequired { pending, evidence } => {
            let risk = match pending.risk {
                RiskLevel::High => ApprovalRisk::High,
                RiskLevel::Medium => ApprovalRisk::Medium,
                RiskLevel::Low => ApprovalRisk::Low,
            };
            let preview = decode_approval_preview(&pending.tool_name, &pending.payload);
            state.pending_approval = Some(PendingApprovalState {
                tool_name: pending.tool_name,
                summary: pending.summary,
                risk,
                evidence,
                preview,
            });
            state.mark_dirty(DirtySections::INPUT);
            state.set_status("awaiting approval");
        }
        RuntimeEvent::InfoMessage(text) => {
            state.add_collapsible_tool_message(summarize_command_output(&text))
        }
        RuntimeEvent::PromptAssembled(prompt) => state.set_last_prompt(prompt),
        RuntimeEvent::SystemMessage(text) => state.add_system_message(text),
        RuntimeEvent::FileReadFinished {
            path,
            line_count,
            content: _,
        } => {
            state.add_system_message(format!(
                "read {path} ({line_count} lines) — Ctrl+O to expand"
            ));
        }
        RuntimeEvent::DirectReadCompleted => {
            let message_index = state.messages.len() - 1;
            state.store_file_read(message_index);
        }
        RuntimeEvent::ContextUsage {
            prompt_tokens,
            context_window_tokens,
        } => {
            let pct = (prompt_tokens * 100 / u64::from(context_window_tokens)).min(100) as u8;
            state.set_context_pct(pct);
        }
        // Advisory only — absorbed by the logging layer before reaching here.
        RuntimeEvent::BackendTiming { .. } => {}
        RuntimeEvent::BackendTokenCounts { .. } => {}
        RuntimeEvent::RuntimeTrace(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::app::paths::AppPaths;
    use crate::core::config::Config;
    use crate::runtime::{AnswerSource, RuntimeEvent};
    use crate::tools::{PendingAction, RiskLevel};
    use crate::tui::state::{AppState, ApprovalRisk};

    use super::{apply_runtime_event, decode_approval_preview};

    fn make_state() -> AppState {
        let config = Config::default();
        let paths = AppPaths {
            root_dir: PathBuf::from("/tmp"),
            project_root: PathBuf::from("/tmp"),
            config_file: PathBuf::from("/tmp/config.toml"),
            data_dir: PathBuf::from("/tmp/data"),
            logs_dir: PathBuf::from("/tmp/logs"),
            session_db: PathBuf::from("/tmp/data/sessions.db"),
        };
        AppState::new(&config, &paths)
    }

    fn make_pending(tool_name: &str, risk: RiskLevel) -> PendingAction {
        PendingAction {
            tool_name: tool_name.to_string(),
            summary: format!("{tool_name} summary"),
            risk,
            payload: String::new(),
        }
    }

    #[test]
    fn context_usage_event_sets_context_pct() {
        let mut state = make_state();
        assert_eq!(state.context_pct, None, "starts with no indicator");

        apply_runtime_event(
            &mut state,
            RuntimeEvent::ContextUsage {
                prompt_tokens: 64_000,
                context_window_tokens: 128_000,
            },
        );

        assert_eq!(state.context_pct, Some(50));
    }

    #[test]
    fn context_usage_event_clamps_at_100_pct() {
        let mut state = make_state();

        apply_runtime_event(
            &mut state,
            RuntimeEvent::ContextUsage {
                prompt_tokens: 200_000,
                context_window_tokens: 128_000,
            },
        );

        assert_eq!(state.context_pct, Some(100));
    }

    #[test]
    fn approval_required_sets_pending_approval() {
        let mut state = make_state();
        let messages_before = state.messages.len();

        apply_runtime_event(
            &mut state,
            RuntimeEvent::ApprovalRequired {
                pending: make_pending("shell", RiskLevel::High),
                evidence: vec!["src/main.rs:10".to_string()],
            },
        );

        let approval = state.pending_approval.as_ref().expect("should be Some");
        assert_eq!(approval.tool_name, "shell");
        assert_eq!(approval.summary, "shell summary");
        assert_eq!(approval.risk, ApprovalRisk::High);
        assert_eq!(approval.evidence, vec!["src/main.rs:10"]);
        assert_eq!(state.status, "awaiting approval");
        assert_eq!(
            state.messages.len(),
            messages_before,
            "no transcript entry added"
        );
    }

    #[test]
    fn approval_required_maps_medium_risk() {
        let mut state = make_state();

        apply_runtime_event(
            &mut state,
            RuntimeEvent::ApprovalRequired {
                pending: make_pending("edit_file", RiskLevel::Medium),
                evidence: vec![],
            },
        );

        let approval = state.pending_approval.as_ref().unwrap();
        assert_eq!(approval.risk, ApprovalRisk::Medium);
    }

    #[test]
    fn answer_ready_clears_is_busy() {
        let mut state = make_state();
        state.is_busy = true;
        apply_runtime_event(&mut state, RuntimeEvent::AnswerReady(AnswerSource::Direct));
        assert!(!state.is_busy, "AnswerReady must clear is_busy");
    }

    #[test]
    fn failed_clears_is_busy() {
        let mut state = make_state();
        state.is_busy = true;
        apply_runtime_event(
            &mut state,
            RuntimeEvent::Failed {
                message: "err".into(),
            },
        );
        assert!(!state.is_busy, "Failed must clear is_busy");
    }

    #[test]
    fn answer_ready_clears_pending_approval() {
        let mut state = make_state();
        apply_runtime_event(
            &mut state,
            RuntimeEvent::ApprovalRequired {
                pending: make_pending("shell", RiskLevel::High),
                evidence: vec![],
            },
        );
        assert!(state.pending_approval.is_some());

        apply_runtime_event(&mut state, RuntimeEvent::AnswerReady(AnswerSource::Direct));
        assert!(
            state.pending_approval.is_none(),
            "AnswerReady must clear pending_approval"
        );
    }

    #[test]
    fn failed_clears_pending_approval() {
        let mut state = make_state();
        apply_runtime_event(
            &mut state,
            RuntimeEvent::ApprovalRequired {
                pending: make_pending("edit_file", RiskLevel::Medium),
                evidence: vec![],
            },
        );
        assert!(state.pending_approval.is_some());

        apply_runtime_event(
            &mut state,
            RuntimeEvent::Failed {
                message: "err".into(),
            },
        );
        assert!(
            state.pending_approval.is_none(),
            "Failed must clear pending_approval"
        );
    }

    #[test]
    fn decode_edit_file_produces_diff_lines() {
        let payload = "v2\x00/abs/src/lib.rs\x00src/lib.rs\x00old line\x00new line";
        let preview = decode_approval_preview("edit_file", payload);
        assert_eq!(preview, vec!["- old line", "+ new line"]);
    }

    #[test]
    fn decode_edit_file_caps_at_four_lines() {
        let search = "a\nb\nc";
        let replace = "x\ny\nz";
        let payload = format!("v2\x00/abs/f.rs\x00f.rs\x00{search}\x00{replace}");
        let preview = decode_approval_preview("edit_file", &payload);
        assert_eq!(preview.len(), 4, "must cap at 4 total lines");
        assert!(preview[0].starts_with("- "));
        assert!(preview[1].starts_with("- "));
        assert!(preview[2].starts_with("- "));
        assert!(preview[3].starts_with("+ "));
    }

    #[test]
    fn decode_shell_produces_command_line() {
        let preview = decode_approval_preview("shell", "cargo test --no-default-features");
        assert_eq!(preview, vec!["cargo test --no-default-features"]);
    }

    #[test]
    fn decode_write_file_produces_indented_content_lines() {
        let payload = "v2\x00/abs/out.rs\x00out.rs\x00fn main() {}\nfn foo() {}\nfn bar() {}";
        let preview = decode_approval_preview("write_file", payload);
        assert_eq!(
            preview,
            vec!["  fn main() {}", "  fn foo() {}", "  fn bar() {}"]
        );
    }

    #[test]
    fn decode_unknown_tool_produces_empty_preview() {
        let preview = decode_approval_preview("read_file", "some payload");
        assert!(preview.is_empty());
    }

    #[test]
    fn decode_empty_payload_does_not_panic() {
        assert!(decode_approval_preview("edit_file", "").is_empty());
        assert!(decode_approval_preview("shell", "").is_empty());
        assert!(decode_approval_preview("write_file", "").is_empty());
    }
}
