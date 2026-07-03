pub mod backend;
pub mod cli;
pub mod config;
pub mod context;
pub(crate) mod dto;
pub mod error;
pub mod paths;
pub mod session;

pub use context::AppContext;
pub use error::{AppError, Result};

use crate::llm::providers::build_backend;
use crate::tools::default_registry;
use crate::tui;

// Bootstraps the application: prepares paths and config, builds the backend and tools, restores session state,
// attaches logging, and starts the TUI.
pub fn run(cli: cli::Cli, start_dir: Option<std::path::PathBuf>) -> Result<()> {
    let paths = paths::AppPaths::discover(start_dir)?;
    paths.ensure_runtime_dirs()?;
    load_dotenv(&paths.project_root);

    let mut config = config::load(&paths.config_file)?.resolve_paths(&paths.root_dir);
    if let Some(model) = cli.model {
        config.llm.provider = model;
    }
    let backend = build_backend(&config)?;
    let project_root = crate::runtime::ProjectRoot::new(paths.project_root.clone())
        .map_err(|e| AppError::Config(e.to_string()))?;
    let registry = default_registry().with_project_root(project_root.as_path_buf());
    let log = crate::logging::SessionLog::open(&paths.logs_dir);
    let thunk_md = read_thunk_md(&paths.thunk_dir, &paths.project_root);

    let (active_session, history, anchors) =
        session::ActiveSession::open_or_restore(&paths.session_db, &project_root)?;
    let app = AppContext::build(
        &config,
        project_root,
        backend,
        registry,
        active_session,
        history,
        anchors,
        log,
        Some(&paths.session_db),
        thunk_md,
        paths.thunk_dir.clone(),
    )?;

    #[cfg(feature = "gui")]
    if cli.gui {
        return crate::gui::run(&config, &paths, app);
    }
    #[cfg(not(feature = "gui"))]
    if cli.gui {
        eprintln!("GUI support not compiled in. Rebuild with --features gui.");
        std::process::exit(1);
    }

    tui::run(&config, &paths, app)
}

fn read_thunk_md(thunk_dir: &std::path::Path, project_root: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(thunk_dir.join("THUNK.md"))
        .or_else(|_| std::fs::read_to_string(project_root.join("THUNK.md")))
        .ok()
}

fn load_dotenv(project_root: &std::path::Path) {
    let env_path = project_root.join(".env");
    let Ok(contents) = std::fs::read_to_string(&env_path) else {
        return;
    };
    let mut loaded = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            if std::env::var(key).is_err() {
                std::env::set_var(key, value);
                loaded.push(key.to_string());
            }
        }
    }
    if !loaded.is_empty() {
        eprintln!("[thunk] loaded .env: {}", loaded.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::load_dotenv;
    use super::read_thunk_md;

    #[test]
    fn load_dotenv_parses_key_value_comments_blanks_and_quoted_values() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "# comment line\n\nPLAIN_KEY=plain_value\nDQ_KEY=\"double quoted\"\nSQ_KEY='single quoted'\n",
        )
        .unwrap();

        let plain_key = "THUNK_TEST_PLAIN_KEY_9a3f";
        let dq_key = "THUNK_TEST_DQ_KEY_9a3f";
        let sq_key = "THUNK_TEST_SQ_KEY_9a3f";

        fs::write(
            dir.path().join(".env"),
            format!(
                "# comment\n\n{plain_key}=plain_value\n{dq_key}=\"double quoted\"\n{sq_key}='single quoted'\n"
            ),
        )
        .unwrap();

        // Ensure keys are absent before loading.
        std::env::remove_var(plain_key);
        std::env::remove_var(dq_key);
        std::env::remove_var(sq_key);

        load_dotenv(dir.path());

        assert_eq!(std::env::var(plain_key).unwrap(), "plain_value");
        assert_eq!(std::env::var(dq_key).unwrap(), "double quoted");
        assert_eq!(std::env::var(sq_key).unwrap(), "single quoted");
    }

    #[test]
    fn load_dotenv_does_not_override_existing_env_vars() {
        let dir = tempdir().unwrap();
        let key = "THUNK_TEST_NO_OVERRIDE_9a3f";
        std::env::set_var(key, "original");
        fs::write(dir.path().join(".env"), format!("{key}=new_value\n")).unwrap();

        load_dotenv(dir.path());

        assert_eq!(std::env::var(key).unwrap(), "original");
        std::env::remove_var(key);
    }

    #[test]
    fn load_dotenv_missing_env_file_is_silent() {
        let dir = tempdir().unwrap();
        // No .env file — should not panic.
        load_dotenv(dir.path());
    }

    #[test]
    fn read_thunk_md_prefers_thunk_dir_over_root() {
        let dir = tempdir().unwrap();
        let thunk_dir = dir.path().join(".thunk");
        fs::create_dir_all(&thunk_dir).unwrap();
        fs::write(thunk_dir.join("THUNK.md"), "from thunk dir").unwrap();
        fs::write(dir.path().join("THUNK.md"), "from root").unwrap();

        let result = read_thunk_md(&thunk_dir, dir.path());
        assert_eq!(result.as_deref(), Some("from thunk dir"));
    }

    #[test]
    fn read_thunk_md_falls_back_to_project_root() {
        let dir = tempdir().unwrap();
        let thunk_dir = dir.path().join(".thunk");
        fs::create_dir_all(&thunk_dir).unwrap();
        fs::write(dir.path().join("THUNK.md"), "from root").unwrap();

        let result = read_thunk_md(&thunk_dir, dir.path());
        assert_eq!(result.as_deref(), Some("from root"));
    }

    #[test]
    fn read_thunk_md_absent_returns_none() {
        let dir = tempdir().unwrap();
        let thunk_dir = dir.path().join(".thunk");

        let result = read_thunk_md(&thunk_dir, dir.path());
        assert!(result.is_none());
    }
}
