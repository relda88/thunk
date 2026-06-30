use std::sync::mpsc;

use crate::core::error::Result;
use crate::runtime::RuntimeRequest;

use super::super::state::AppState;
use super::super::worker::WorkerCmd;
use super::{help_text, resolve_command, Command, CommandAction};

pub(crate) fn dispatch_command_runtime_request(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    req: RuntimeRequest,
) -> Result<()> {
    if state.is_busy {
        return Ok(());
    }
    state.is_busy = true;
    let _ = cmd_tx.send(WorkerCmd::Handle(req));
    Ok(())
}

pub(crate) fn submit_to_app(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    prompt: String,
) -> Result<()> {
    if state.is_busy {
        return Ok(());
    }
    state.add_user_message(prompt.clone());
    state.is_busy = true;
    let _ = cmd_tx.send(WorkerCmd::Handle(RuntimeRequest::Submit { text: prompt }));
    Ok(())
}

pub(crate) fn handle_command(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    cmd: Command,
) -> Result<()> {
    match resolve_command(cmd) {
        CommandAction::ShowHelp => {
            state.add_system_message(help_text());
        }
        CommandAction::Quit => {
            state.should_quit = true;
        }
        CommandAction::ClearSession => {
            if state.is_busy {
                return Ok(());
            }
            state.clear_messages();
            state.is_busy = true;
            let _ = cmd_tx.send(WorkerCmd::Reset);
        }
        CommandAction::ListSessions => {
            if state.is_busy {
                return Ok(());
            }
            state.is_busy = true;
            let _ = cmd_tx.send(WorkerCmd::ListSessions);
        }
        CommandAction::ClearProjectSessions => {
            if state.is_busy {
                return Ok(());
            }
            state.clear_messages();
            state.is_busy = true;
            let _ = cmd_tx.send(WorkerCmd::ClearSessions);
        }
        CommandAction::Runtime(req) => {
            dispatch_command_runtime_request(state, cmd_tx, req)?;
        }
    }
    Ok(())
}
