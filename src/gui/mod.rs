mod commands;

use std::sync::Mutex;

use tauri::{Emitter, Manager};

use crate::app::backend::spawn_backend;
use crate::app::config::Config;
use crate::app::context::AppContext;
use crate::app::dto::worker_reply_to_dto;
use crate::app::paths::AppPaths;
use crate::core::error::{AppError, Result};

pub(crate) fn run(config: &Config, paths: &AppPaths, app: AppContext) -> Result<()> {
    let config = config.clone();
    let paths = paths.clone();

    tauri::Builder::default()
        .setup(move |tauri_app| {
            let backend = spawn_backend(app, &config, &paths);
            let cmd_tx = backend.cmd_tx;
            let reply_rx = backend.reply_rx;

            tauri_app.manage(Mutex::new(cmd_tx));

            let handle = tauri_app.handle().clone();
            std::thread::spawn(move || {
                for reply in reply_rx {
                    if let Some(dto) = worker_reply_to_dto(reply) {
                        let _ = handle.emit("runtime-event", &dto);
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::submit])
        .run(tauri::generate_context!())
        .map_err(|e| AppError::Tui(e.to_string()))
}
