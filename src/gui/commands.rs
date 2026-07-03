use std::sync::{mpsc::Sender, Mutex};

use serde::Serialize;

use crate::core::config::Config;
use crate::runtime::RuntimeRequest;
use crate::tui::commands::{self, CommandAction, ParseError};
use crate::tui::worker::WorkerCmd;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AppInfo {
    pub project_label: Option<String>,
    pub app_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HelpCommandDto {
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub(crate) fn get_help_commands() -> Vec<HelpCommandDto> {
    commands::launcher_commands()
        .iter()
        .map(|c| HelpCommandDto {
            name: c.name.to_string(),
            description: c.description.to_string(),
        })
        .collect()
}

#[tauri::command]
pub(crate) fn submit(
    text: String,
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::Submit { text }))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn approve(state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::Approve))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn reject(state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::Reject))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn plan_approve(
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::PlanApprove))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn plan_abandon(
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::PlanAbandon))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn memory_approve(
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::MemoryApprove))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn memory_reject(
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::MemoryReject))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn app_info(state: tauri::State<'_, AppInfo>) -> AppInfo {
    state.inner().clone()
}

#[tauri::command]
pub(crate) fn run_command(
    input: String,
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
    config: tauri::State<'_, Config>,
) -> Result<(), String> {
    let parsed = match commands::parse(&input) {
        None => return Ok(()),
        Some(Ok(cmd)) => cmd,
        Some(Err(ParseError::UnknownCommand)) => {
            match commands::resolve_custom_command(&config, &input) {
                None => return Err(commands::help_text().to_string()),
                Some(Ok(req)) => {
                    let tx = state.lock().map_err(|e| e.to_string())?;
                    return tx.send(WorkerCmd::Handle(req)).map_err(|e| e.to_string());
                }
                Some(Err(msg)) => return Err(msg),
            }
        }
        Some(Err(e)) => return Err(e.user_message()),
    };

    match commands::resolve_command(parsed) {
        CommandAction::Runtime(req) => {
            let tx = state.lock().map_err(|e| e.to_string())?;
            tx.send(WorkerCmd::Handle(req)).map_err(|e| e.to_string())
        }
        CommandAction::ClearSession => {
            let tx = state.lock().map_err(|e| e.to_string())?;
            tx.send(WorkerCmd::Reset).map_err(|e| e.to_string())
        }
        CommandAction::ListSessions => {
            let tx = state.lock().map_err(|e| e.to_string())?;
            tx.send(WorkerCmd::ListSessions).map_err(|e| e.to_string())
        }
        CommandAction::ClearProjectSessions => {
            let tx = state.lock().map_err(|e| e.to_string())?;
            tx.send(WorkerCmd::ClearSessions).map_err(|e| e.to_string())
        }
        // The frontend intercepts "/help" and calls `get_help_commands` directly,
        // so this arm is unreachable from the GUI in practice.
        CommandAction::ShowHelp => Ok(()),
        CommandAction::Quit => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_help_commands_mirrors_launcher_commands() {
        let launcher = commands::launcher_commands();
        let dtos = get_help_commands();

        assert_eq!(dtos.len(), launcher.len());
        for (dto, entry) in dtos.iter().zip(launcher.iter()) {
            assert_eq!(dto.name, entry.name);
            assert_eq!(dto.description, entry.description);
        }
    }
}
