mod app;
pub(crate) mod collapsible;
pub mod commands;
mod cursor;
mod events;
mod format;
mod input;
mod keybindings;
mod renderer;
mod state;
mod worker;

use std::io::{self, IsTerminal};

use crossterm::{
    cursor::{Hide, SetCursorStyle, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen, SetTitle,
    },
};

use crate::app::config::Config;
use crate::app::context::AppContext;
use crate::app::paths::AppPaths;
use crate::core::error::{AppError, Result};

/// Main entry point for the TUI, handling terminal setup and teardown
pub fn run(config: &Config, paths: &AppPaths, app: AppContext) -> Result<()> {
    if !io::stdout().is_terminal() {
        return Err(AppError::Tui(
            "The TUI requires an interactive terminal (stdout is not a TTY).".to_string(),
        ));
    }

    if std::env::var("TERM").as_deref() == Ok("dumb") {
        return Err(AppError::Tui(
            "The TUI cannot run with TERM=dumb.".to_string(),
        ));
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        Clear(ClearType::All),
        EnableBracketedPaste,
        Hide,
        SetCursorStyle::SteadyBar,
        SetTitle(config.app.name.as_str())
    )?;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app::run_app(&mut stdout, config, paths, app)
    }));

    disable_raw_mode()?;
    execute!(
        stdout,
        LeaveAlternateScreen,
        DisableBracketedPaste,
        Show,
        SetCursorStyle::DefaultUserShape,
        SetTitle(config.app.name.as_str())
    )?;

    match result {
        Ok(result) => result,
        Err(_) => Err(AppError::Tui(
            "The TUI panicked unexpectedly after startup. Terminal state was restored.".to_string(),
        )),
    }
}
