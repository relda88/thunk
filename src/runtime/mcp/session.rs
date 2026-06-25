use std::process::{Child, ChildStdin};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::core::error::{AppError, Result};

use super::transport::{recv_response, send_notification, send_request, spawn_server};
use super::types::McpServerConfig;

pub(super) struct McpSession {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
    timeout: Duration,
    next_id: u64,
    pub server_name: String,
}

impl McpSession {
    /// Spawns the MCP server process and completes the initialize handshake.
    /// Kills the child and returns Err if the handshake fails.
    pub(super) fn start(config: &McpServerConfig, timeout: Duration) -> Result<Self> {
        let (mut child, mut stdin, rx) = spawn_server(config)?;

        let result = (|| -> Result<()> {
            send_request(
                &mut stdin,
                1,
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "thunk",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )?;
            let response = recv_response(&rx, 1, timeout)?;
            if let Some(err) = response.get("error") {
                return Err(AppError::Tool(format!(
                    "MCP initialize failed for '{}': {err}",
                    config.name
                )));
            }
            send_notification(&mut stdin, "notifications/initialized", json!({}))?;
            Ok(())
        })();

        if let Err(e) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }

        Ok(Self {
            child,
            stdin,
            rx,
            timeout,
            next_id: 2,
            server_name: config.name.clone(),
        })
    }

    pub(super) fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Sends a request and waits for the response. On any failure (timeout or crash),
    /// kills the child process and returns Err — the session is dead and should be removed.
    pub(super) fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let result = send_request(&mut self.stdin, id, method, params)
            .and_then(|()| recv_response(&self.rx, id, self.timeout));

        if result.is_err() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }

        result
    }

    /// Best-effort graceful shutdown: send cancellation notification, then kill+wait.
    pub(super) fn shutdown(&mut self) {
        let _ = send_notification(&mut self.stdin, "notifications/cancelled", json!({}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
