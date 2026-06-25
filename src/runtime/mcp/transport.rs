use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use serde_json::{json, Value};

use crate::core::error::{AppError, Result};

use super::types::McpServerConfig;

pub(super) fn spawn_server(
    config: &McpServerConfig,
) -> Result<(Child, ChildStdin, mpsc::Receiver<Value>)> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    if let Some(env) = &config.env {
        for (key, val) in env {
            cmd.env(key, val);
        }
    }

    let mut child = cmd.spawn().map_err(|e| {
        AppError::Tool(format!(
            "failed to start MCP server '{}' via '{}': {e}",
            config.name, config.command
        ))
    })?;

    let stdin = child.stdin.take().ok_or_else(|| {
        AppError::Tool(format!(
            "failed to open stdin for MCP server '{}'",
            config.name
        ))
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        AppError::Tool(format!(
            "failed to open stdout for MCP server '{}'",
            config.name
        ))
    })?;

    let rx = spawn_reader(stdout);
    Ok((child, stdin, rx))
}

fn spawn_reader(stdout: ChildStdout) -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
                        if tx.send(value).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });
    rx
}

pub(super) fn send_request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<()> {
    let msg = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut payload = serde_json::to_string(&msg)
        .map_err(|e| AppError::Tool(format!("MCP request serialization failed: {e}")))?;
    payload.push('\n');
    stdin.write_all(payload.as_bytes())?;
    stdin.flush()?;
    Ok(())
}

pub(super) fn send_notification(stdin: &mut ChildStdin, method: &str, params: Value) -> Result<()> {
    let msg = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let mut payload = serde_json::to_string(&msg)
        .map_err(|e| AppError::Tool(format!("MCP notification serialization failed: {e}")))?;
    payload.push('\n');
    stdin.write_all(payload.as_bytes())?;
    stdin.flush()?;
    Ok(())
}

/// Drains rx until a response with the given id arrives.
/// Notifications (messages without id) are discarded.
/// Returns Err on timeout or channel disconnect — caller is responsible for killing the child.
pub(super) fn recv_response(
    rx: &mpsc::Receiver<Value>,
    id: u64,
    timeout: Duration,
) -> Result<Value> {
    loop {
        match rx.recv_timeout(timeout) {
            Ok(msg) => {
                // Discard notifications (no "id") and mismatched responses.
                if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return Ok(msg);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(AppError::Tool(format!(
                    "MCP call timed out after {}s",
                    timeout.as_secs()
                )));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AppError::Tool("MCP server crashed".to_string()));
            }
        }
    }
}
