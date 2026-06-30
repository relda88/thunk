use std::sync::{mpsc::Sender, Mutex};

use serde::Serialize;

use crate::runtime::RuntimeRequest;
use crate::tui::worker::WorkerCmd;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AppInfo {
    pub project_label: Option<String>,
    pub app_name: String,
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
