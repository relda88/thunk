use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use serde_json::Value;

use crate::core::error::{AppError, Result};

use super::protocol::{
    format_lsp_response_error, is_retryable_lsp_query_error, parse_definition_response,
    parse_diagnostic, parse_hover_response, parse_lsp_response_error,
};
use super::types::{DefinitionResponse, HoverResponse, LspCommandSpec, LspDiagnostic};

pub(super) fn spawn_language_server(spec: &LspCommandSpec, project_root: &Path) -> Result<Child> {
    Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            AppError::Tool(format!(
                "failed to start LSP server via {}: {e}",
                spec.display
            ))
        })
}

pub(super) fn write_lsp_message(stdin: &mut ChildStdin, value: &Value) -> Result<()> {
    let payload = value.to_string();
    write!(
        stdin,
        "Content-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    )?;
    stdin.flush()?;
    Ok(())
}

pub(super) fn spawn_reader(stdout: ChildStdout) -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Ok(message) = read_lsp_message(&mut reader) {
            if tx.send(message).is_err() {
                break;
            }
        }
    });
    rx
}

fn read_lsp_message(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err(AppError::Tool("LSP session crashed".to_string()));
        }

        if line == "\r\n" || line == "\n" {
            break;
        }

        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                let parsed = value.trim().parse::<usize>().map_err(|e| {
                    AppError::Tool(format!("invalid LSP Content-Length header: {e}"))
                })?;
                content_length = Some(parsed);
            }
        }
    }

    let length = content_length
        .ok_or_else(|| AppError::Tool("missing LSP Content-Length header".to_string()))?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map_err(|e| AppError::Tool(format!("invalid LSP JSON payload: {e}")))
}

/// Receives one message from the channel, mapping the two error cases to distinct AppErrors.
/// Disconnected (server died) → "LSP session crashed" — caller must clear the session.
/// Timeout (server alive but slow) → "LSP timed out" — caller keeps the session alive.
fn recv(rx: &mpsc::Receiver<Value>, timeout: Duration) -> Result<Value> {
    match rx.recv_timeout(timeout) {
        Ok(msg) => Ok(msg),
        Err(RecvTimeoutError::Disconnected) => {
            Err(AppError::Tool("LSP session crashed".to_string()))
        }
        Err(RecvTimeoutError::Timeout) => Err(AppError::Tool(
            "LSP timed out, increase [lsp].timeout_ms in config.toml".to_string(),
        )),
    }
}

pub(super) fn wait_for_response(
    rx: &mpsc::Receiver<Value>,
    id: u64,
    timeout: Duration,
) -> Result<Value> {
    loop {
        let message = recv(rx, timeout)?;
        if message.get("id").and_then(|v| v.as_u64()) == Some(id) {
            if let Some(error) = parse_lsp_response_error(&message) {
                return Err(AppError::Tool(format!(
                    "LSP server error: {}",
                    format_lsp_response_error(&error)
                )));
            }
            return Ok(message);
        }
    }
}

pub(super) fn wait_for_hover_response(
    rx: &mpsc::Receiver<Value>,
    id: u64,
    timeout: Duration,
) -> Result<HoverResponse> {
    loop {
        let message = recv(rx, timeout)?;
        if message.get("id").and_then(|v| v.as_u64()) == Some(id) {
            if let Some(error) = parse_lsp_response_error(&message) {
                if is_retryable_lsp_query_error(&error) {
                    return Ok(HoverResponse::RetryableError(format_lsp_response_error(
                        &error,
                    )));
                }
                return Err(AppError::Tool(format!(
                    "LSP server error: {}",
                    format_lsp_response_error(&error)
                )));
            }
            return Ok(match parse_hover_response(&message) {
                Some(text) if !text.trim().is_empty() => HoverResponse::Hover(text),
                _ => HoverResponse::NoInfo,
            });
        }
    }
}

pub(super) fn wait_for_definition_response(
    rx: &mpsc::Receiver<Value>,
    id: u64,
    timeout: Duration,
) -> Result<DefinitionResponse> {
    loop {
        let message = recv(rx, timeout)?;
        if message.get("id").and_then(|v| v.as_u64()) == Some(id) {
            if let Some(error) = parse_lsp_response_error(&message) {
                if is_retryable_lsp_query_error(&error) {
                    return Ok(DefinitionResponse::RetryableError(
                        format_lsp_response_error(&error),
                    ));
                }
                return Err(AppError::Tool(format!(
                    "LSP server error: {}",
                    format_lsp_response_error(&error)
                )));
            }
            let definitions = parse_definition_response(&message);
            return Ok(if definitions.is_empty() {
                DefinitionResponse::NoInfo
            } else {
                DefinitionResponse::Definitions(definitions)
            });
        }
    }
}

pub(super) fn wait_for_diagnostics(
    rx: &mpsc::Receiver<Value>,
    target_uri: &str,
    timeout: Duration,
) -> Result<Vec<LspDiagnostic>> {
    loop {
        let message = recv(rx, timeout)?;
        if message.get("method").and_then(|v| v.as_str()) == Some("textDocument/publishDiagnostics")
        {
            let params = &message["params"];
            if params["uri"].as_str() == Some(target_uri) {
                let diagnostics = params["diagnostics"]
                    .as_array()
                    .map(|items| items.iter().filter_map(parse_diagnostic).collect())
                    .unwrap_or_default();
                return Ok(diagnostics);
            }
        }
    }
}
