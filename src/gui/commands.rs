use std::sync::{mpsc::Sender, Mutex};

use crate::runtime::RuntimeRequest;
use crate::tui::worker::WorkerCmd;

#[tauri::command]
pub(crate) fn submit(
    text: String,
    state: tauri::State<'_, Mutex<Sender<WorkerCmd>>>,
) -> Result<(), String> {
    let tx = state.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCmd::Handle(RuntimeRequest::Submit { text }))
        .map_err(|e| e.to_string())
}
