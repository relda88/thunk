use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, ChildStdin};
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use serde_json::{json, Value};

use crate::core::error::{AppError, Result};

use super::paths::path_to_file_uri;
use super::position::{build_hover_positions, line_column_to_utf16};
use super::transport::{
    spawn_language_server, spawn_reader, wait_for_definition_response, wait_for_diagnostics,
    wait_for_hover_response, wait_for_response, write_lsp_message,
};
use super::types::{
    DefinitionLocation, DefinitionResponse, HoverResponse, LspCommandSpec, LspDiagnostic,
};

pub(super) struct LspSession {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
    timeout: Duration,
    next_id: u64,
    open_files: HashMap<String, u32>,
}

impl LspSession {
    /// Spawns the LSP server, completes the initialize handshake, then blocks on
    /// `startup_timeout` waiting for the first `publishDiagnostics` notification.
    /// This absorbs the initial indexing delay once so subsequent queries are fast.
    /// If `startup_timeout` expires the session is kept alive — queries handle
    /// retryable errors from the server's still-indexing state.
    pub(super) fn start(
        spec: &LspCommandSpec,
        project_root: &Path,
        timeout: Duration,
        startup_timeout: Duration,
    ) -> Result<Self> {
        let mut child = spawn_language_server(spec, project_root)?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            AppError::Tool("failed to open LSP server stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::Tool("failed to open LSP server stdout".to_string())
        })?;
        let rx = spawn_reader(stdout);

        let root_uri = path_to_file_uri(project_root);
        let workspace_name = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");

        write_lsp_message(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": serde_json::Value::Null,
                    "rootUri": root_uri,
                    "workspaceFolders": [{ "uri": root_uri, "name": workspace_name }],
                    "capabilities": {},
                    "clientInfo": {
                        "name": "thunk",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
        )?;
        wait_for_response(&rx, 1, timeout)?;

        write_lsp_message(
            &mut stdin,
            &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        )?;

        wait_until_ready(&rx, startup_timeout)?;

        Ok(Self {
            child,
            stdin,
            rx,
            timeout,
            next_id: 2,
            open_files: HashMap::new(),
        })
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Ensures `file_uri` is open in the server. First open sends `didOpen`;
    /// subsequent calls for the same URI send `didChange` with an incremented version,
    /// keeping the server's view of the file in sync with the current `source`.
    fn ensure_file_open(&mut self, file_uri: &str, source: &str) -> Result<()> {
        if let Some(version) = self.open_files.get_mut(file_uri) {
            *version += 1;
            let v = *version;
            write_lsp_message(
                &mut self.stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": file_uri, "version": v },
                        "contentChanges": [{ "text": source }]
                    }
                }),
            )?;
        } else {
            write_lsp_message(
                &mut self.stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": file_uri,
                            "languageId": "rust",
                            "version": 1,
                            "text": source
                        }
                    }
                }),
            )?;
            self.open_files.insert(file_uri.to_string(), 1);
        }
        Ok(())
    }

    pub(super) fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub(super) fn definition(
        &mut self,
        file_path: &Path,
        source: &str,
        line: usize,
        column: usize,
    ) -> Result<Vec<DefinitionLocation>> {
        let file_uri = path_to_file_uri(file_path);
        self.ensure_file_open(&file_uri, source)?;

        let hover_positions = build_hover_positions(source, line, column)?;
        let mut definitions = Vec::new();

        for position in hover_positions {
            for _ in 0..3 {
                let utf16_col = line_column_to_utf16(source, position.line, position.column)?;
                let id = self.next_id();
                write_lsp_message(
                    &mut self.stdin,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "textDocument/definition",
                        "params": {
                            "textDocument": { "uri": file_uri },
                            "position": {
                                "line": position.line.saturating_sub(1),
                                "character": utf16_col
                            }
                        }
                    }),
                )?;

                match wait_for_definition_response(&self.rx, id, self.timeout)? {
                    DefinitionResponse::Definitions(items) => {
                        definitions = items;
                    }
                    DefinitionResponse::NoInfo => {}
                    DefinitionResponse::RetryableError(_) => {
                        std::thread::sleep(Duration::from_millis(75));
                        continue;
                    }
                }

                break;
            }

            if !definitions.is_empty() {
                break;
            }
        }

        Ok(definitions)
    }

    pub(super) fn hover(
        &mut self,
        file_path: &Path,
        source: &str,
        line: usize,
        column: usize,
    ) -> Result<Option<String>> {
        let file_uri = path_to_file_uri(file_path);
        self.ensure_file_open(&file_uri, source)?;

        let hover_positions = build_hover_positions(source, line, column)?;
        let mut hover = None;

        for position in hover_positions {
            for _ in 0..3 {
                let utf16_col = line_column_to_utf16(source, position.line, position.column)?;
                let id = self.next_id();
                write_lsp_message(
                    &mut self.stdin,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "textDocument/hover",
                        "params": {
                            "textDocument": { "uri": file_uri },
                            "position": {
                                "line": position.line.saturating_sub(1),
                                "character": utf16_col
                            }
                        }
                    }),
                )?;

                match wait_for_hover_response(&self.rx, id, self.timeout)? {
                    HoverResponse::Hover(text) => {
                        hover = Some(text);
                    }
                    HoverResponse::NoInfo => {}
                    HoverResponse::RetryableError(_) => {
                        std::thread::sleep(Duration::from_millis(75));
                        continue;
                    }
                }

                break;
            }

            if hover.is_some() {
                break;
            }
        }

        Ok(hover)
    }

    pub(super) fn diagnostics(
        &mut self,
        file_path: &Path,
        source: &str,
    ) -> Result<Vec<LspDiagnostic>> {
        let file_uri = path_to_file_uri(file_path);
        self.ensure_file_open(&file_uri, source)?;
        wait_for_diagnostics(&self.rx, &file_uri, self.timeout)
    }

    /// Graceful shutdown: send `shutdown` request (300ms bounded), then `exit` notification,
    /// then kill + wait. Matches LSP spec ordering.
    pub(super) fn close(&mut self) {
        let id = self.next_id();
        let _ = write_lsp_message(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "shutdown",
                "params": serde_json::Value::Null
            }),
        );
        let _ = wait_for_response(&self.rx, id, Duration::from_millis(300));
        let _ = write_lsp_message(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": "exit",
                "params": serde_json::Value::Null
            }),
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LspSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Drains messages until the first `textDocument/publishDiagnostics` notification,
/// which signals that the server has completed enough indexing to serve queries.
/// Returns `Ok(())` on first diagnostics notification OR on timeout (server alive but slow).
/// Returns `Err` only if the server process died (channel disconnected).
fn wait_until_ready(rx: &mpsc::Receiver<Value>, startup_timeout: Duration) -> Result<()> {
    loop {
        match rx.recv_timeout(startup_timeout) {
            Ok(message) => {
                if message.get("method").and_then(|v| v.as_str())
                    == Some("textDocument/publishDiagnostics")
                {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AppError::Tool(
                    "LSP session crashed during startup".to_string(),
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
                return Ok(());
            }
        }
    }
}
