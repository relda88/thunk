use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::config::Config;
use crate::app::Result;
use crate::runtime::RuntimeRequest;

use super::commands;
use super::commands::dispatch;
use super::format;
use super::state::AppState;
use super::worker::WorkerCmd;

pub(super) fn handle_key_event(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    config: &Config,
    key: KeyEvent,
) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }
        (KeyCode::Enter, KeyModifiers::ALT) => state.insert_newline(),
        (KeyCode::Esc, _) if state.is_reverse_search_active() => state.cancel_reverse_search(),
        (KeyCode::Enter, _) if state.is_reverse_search_active() => state.accept_reverse_search(),
        (KeyCode::Backspace, _) if state.is_reverse_search_active() => {
            state.reverse_search_backspace()
        }
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT)
            if state.is_reverse_search_active() =>
        {
            state.reverse_search_push_char(c)
        }
        (KeyCode::Enter, _) => {
            if let Some(input) = state.submit_input() {
                match commands::parse(&input) {
                    None => dispatch::submit_to_app(state, cmd_tx, input)?,
                    Some(Ok(cmd)) => dispatch::handle_command(state, cmd_tx, cmd)?,
                    Some(Err(commands::ParseError::UnknownCommand)) => {
                        match dispatch::resolve_custom_command(config, &input) {
                            None => state.add_system_message(
                                commands::ParseError::UnknownCommand.user_message(),
                            ),
                            Some(Err(msg)) => state.add_system_message(msg),
                            Some(Ok(req)) => {
                                dispatch::dispatch_command_runtime_request(state, cmd_tx, req)?
                            }
                        }
                    }
                    Some(Err(e)) => state.add_system_message(e.user_message()),
                }
            }
        }
        (KeyCode::Backspace, KeyModifiers::ALT) => state.delete_word_before(),
        (KeyCode::Backspace, _) => state.delete_char_before(),
        (KeyCode::Left, _) => state.cursor_left(),
        (KeyCode::Right, _) => state.cursor_right(),
        (KeyCode::Home, _) => state.cursor_home(),
        (KeyCode::End, _) => state.cursor_end(),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            if let Some(prompt) = &state.last_prompt {
                let path = std::env::temp_dir().join("thunk_last_prompt.txt");
                format::dump_prompt_to_file(&path, prompt);
                state.set_status(&format!("prompt dumped to {}", path.display()));
            } else {
                state.set_status("no prompt captured yet");
            }
        }
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => state.recall_previous_input(),
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if state.pending_approval.is_some() {
                dispatch::dispatch_command_runtime_request(state, cmd_tx, RuntimeRequest::Reject)?;
            } else {
                state.recall_next_input();
            }
        }
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
            if state.pending_approval.is_some() {
                dispatch::dispatch_command_runtime_request(state, cmd_tx, RuntimeRequest::Approve)?;
            }
        }
        (KeyCode::Up, _) => state.scroll_up(1),
        (KeyCode::Down, _) => state.scroll_down(1),
        (KeyCode::PageUp, _) => state.scroll_up(10),
        (KeyCode::PageDown, _) => state.scroll_down(10),
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => state.toggle_file_expand(),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => state.delete_word_before(),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => state.reverse_search_cycle(),
        (KeyCode::Char('['), KeyModifiers::ALT) => state.focus_prev_collapsible(),
        (KeyCode::Char(']'), KeyModifiers::ALT) => state.focus_next_collapsible(),
        (KeyCode::Char('o'), KeyModifiers::ALT) => state.toggle_collapse_focused(),
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => state.insert_char(c),
        _ => {}
    }

    Ok(())
}
