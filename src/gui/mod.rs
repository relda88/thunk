use crate::app::config::Config;
use crate::app::context::AppContext;
use crate::app::paths::AppPaths;
use crate::core::error::{AppError, Result};

pub(crate) fn run(_config: &Config, _paths: &AppPaths, _app: AppContext) -> Result<()> {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .map_err(|e| AppError::Tui(e.to_string()))
}
