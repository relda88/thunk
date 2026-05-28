use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::config::LspConfig;
use crate::core::error::{AppError, Result};

use super::probe::resolve_rust_analyzer_command;
use super::session::LspSession;
use super::types::{DefinitionLocation, LspDiagnostic};

pub struct LspManager {
    session: Option<LspSession>,
    config: LspConfig,
    project_root: PathBuf,
}

impl LspManager {
    pub fn new(config: &LspConfig, project_root: &Path) -> Self {
        Self {
            session: None,
            config: config.clone(),
            project_root: project_root.to_path_buf(),
        }
    }

    /// Starts the LSP server if not already running. Idempotent — no-op when a live session
    /// exists. Returns `Err` if LSP is disabled, the binary is not found, or startup fails.
    /// On failure `self.session` remains `None`; the next call retries probe + spawn.
    pub fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            return Err(AppError::Config(
                "LSP is disabled; set [lsp].enabled = true in config.toml to enable it".to_string(),
            ));
        }

        if let Some(session) = &mut self.session {
            if session.is_alive() {
                return Ok(());
            }
            self.session = None;
        }

        let spec = resolve_rust_analyzer_command(&self.config)?;
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let startup_timeout = Duration::from_millis(self.config.startup_timeout_ms);
        let session = LspSession::start(&spec, &self.project_root, timeout, startup_timeout)?;
        self.session = Some(session);
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn is_running(&mut self) -> bool {
        self.session.as_mut().map_or(false, |s| s.is_alive())
    }

    pub fn query_definition(
        &mut self,
        file_path: &Path,
        source: &str,
        line: usize,
        column: usize,
    ) -> Result<Vec<DefinitionLocation>> {
        self.start()?;
        let session = self.session.as_mut().expect("session set by start");
        let result = session.definition(file_path, source, line, column);
        self.handle_session_result(result)
    }

    pub fn query_hover(
        &mut self,
        file_path: &Path,
        source: &str,
        line: usize,
        column: usize,
    ) -> Result<Option<String>> {
        self.start()?;
        let session = self.session.as_mut().expect("session set by start");
        let result = session.hover(file_path, source, line, column);
        self.handle_session_result(result)
    }

    pub fn query_diagnostics(
        &mut self,
        file_path: &Path,
        source: &str,
    ) -> Result<Vec<LspDiagnostic>> {
        self.start()?;
        let session = self.session.as_mut().expect("session set by start");
        let result = session.diagnostics(file_path, source);
        self.handle_session_result(result)
    }

    /// Sends graceful shutdown to the server and clears the session. Idempotent.
    pub fn shutdown(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.close();
        }
    }

    pub fn health_report(&mut self) -> String {
        if !self.config.enabled {
            return "LSP disabled (lsp.enabled = false in config)".to_string();
        }
        let probe_report = crate::runtime::lsp::probe::rust_lsp_health_report(&self.config);
        if self.is_running() {
            format!("LSP running — rust-analyzer active, session alive\n\nProbe report:\n{probe_report}")
        } else {
            format!("LSP enabled — no active session (not yet started or crashed)\n\nProbe report:\n{probe_report}")
        }
    }

    /// Inspects the error to decide whether the session is still viable.
    /// A "LSP session crashed" error means the server process died — clear the session.
    /// Any other error (timeout, parse failure, server-level error) leaves the session intact.
    fn handle_session_result<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Err(AppError::Tool(ref msg)) = result {
            if msg.starts_with("LSP session crashed") {
                self.session = None;
            }
        }
        result
    }
}
