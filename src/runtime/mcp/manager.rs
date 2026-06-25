use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::core::error::Result;

use super::session::McpSession;
use super::types::{McpCallResult, McpConfig, McpServerConfig, McpTool};

pub(crate) struct MCPManager {
    sessions: HashMap<String, McpSession>,
    config: McpConfig,
    timeout: Duration,
}

/// Namespaces a server's raw tool name as `mcp::<server>::<tool>` so discovered MCP
/// tools never collide with static tool names. The `::` separator is parser-safe.
fn namespaced_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp::{server_name}::{tool_name}")
}

/// Builds the `tools/call` params object: `{"name": <bare>, "arguments": <args>}`.
/// `name` is the bare tool name (no `mcp::<server>::` prefix); the MCP protocol
/// addresses tools by their bare name within a server.
fn build_call_params(bare_tool_name: &str, args: Value) -> Value {
    serde_json::json!({
        "name": bare_tool_name,
        "arguments": args,
    })
}

/// Parses a JSON-RPC `tools/call` response (the full envelope, as returned by
/// `call`) into an `McpCallResult`. Concatenates all `text` content blocks with
/// newlines and reads the `isError` flag (defaulting to `false`).
fn parse_call_result(response: &Value) -> McpCallResult {
    let content = response["result"]["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| {
                    if b["type"].as_str() == Some("text") {
                        b["text"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let is_error = response["result"]["isError"].as_bool().unwrap_or(false);
    McpCallResult { content, is_error }
}

impl MCPManager {
    pub(crate) fn new(config: McpConfig) -> Self {
        Self {
            sessions: HashMap::new(),
            config,
            timeout: Duration::from_secs(10),
        }
    }

    /// Starts all configured servers. Failures are non-fatal — a failed server is simply absent
    /// from the sessions map. The session proceeds with only the servers that started successfully.
    pub(crate) fn start_all(&mut self) {
        for server in &self.config.servers.clone() {
            match McpSession::start(server, self.timeout) {
                Ok(session) => {
                    self.sessions.insert(server.name.clone(), session);
                }
                Err(e) => {
                    eprintln!("[thunk] MCP server '{}' failed to start: {e}", server.name);
                }
            }
        }
    }

    pub(crate) fn is_alive(&mut self, server_name: &str) -> bool {
        self.sessions
            .get_mut(server_name)
            .is_some_and(|s| s.is_alive())
    }

    /// Calls method on the named server. Attempts a lazy restart if the session is absent or dead.
    /// Removes the session from the map on call failure (the session killed itself in McpSession::call).
    pub(crate) fn call(&mut self, server_name: &str, method: &str, params: Value) -> Result<Value> {
        // Restart if dead or missing.
        if !self.sessions.contains_key(server_name)
            || !self
                .sessions
                .get_mut(server_name)
                .is_some_and(|s| s.is_alive())
        {
            self.sessions.remove(server_name);
            if let Some(config) = self.config.servers.iter().find(|s| s.name == server_name) {
                let config = config.clone();
                match McpSession::start(&config, self.timeout) {
                    Ok(session) => {
                        self.sessions.insert(server_name.to_string(), session);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            } else {
                return Err(crate::core::error::AppError::Tool(format!(
                    "no MCP server named '{server_name}' in config"
                )));
            }
        }

        let session = self.sessions.get_mut(server_name).expect("just inserted");
        let result = session.call(method, params);
        if result.is_err() {
            // Session killed itself on failure — remove it so the next call triggers a restart.
            self.sessions.remove(server_name);
        }
        result
    }

    /// Calls a discovered MCP tool via the `tools/call` JSON-RPC method.
    /// `bare_tool_name` is the un-namespaced tool name (no `mcp::<server>::` prefix);
    /// `args` is the JSON object passed as the tool's `arguments`. Returns the flattened
    /// text content and the server-reported `isError` flag. A tool-level error
    /// (`isError: true`) is NOT an `Err` — it is surfaced to the model as tool output.
    /// Only a protocol/transport failure produces `Err`.
    pub(crate) fn call_tool(
        &mut self,
        server_name: &str,
        bare_tool_name: &str,
        args: Value,
    ) -> Result<McpCallResult> {
        let params = build_call_params(bare_tool_name, args);
        let response = self.call(server_name, "tools/call", params)?;
        Ok(parse_call_result(&response))
    }

    pub(crate) fn shutdown(&mut self) {
        for (_, session) in self.sessions.drain() {
            // McpSession::shutdown requires &mut self; drain gives owned values so Drop runs.
            drop(session);
        }
    }
}

impl MCPManager {
    /// Returns the config server entry for a given server name. Used by callers that need
    /// the server spec without going through the session (e.g. for tool discovery).
    #[allow(dead_code)]
    pub(crate) fn server_config(&self, name: &str) -> Option<&McpServerConfig> {
        self.config.servers.iter().find(|s| s.name == name)
    }

    /// Returns names of all configured servers.
    #[allow(dead_code)]
    pub(crate) fn server_names(&self) -> impl Iterator<Item = &str> {
        self.config.servers.iter().map(|s| s.name.as_str())
    }

    /// Queries a single server for its tool schemas via `tools/list` and returns the
    /// discovered tools with namespaced names (`mcp::<server>::<tool>`).
    /// Per-server error isolation: any failure (dead server, malformed response) yields
    /// an empty vec — it never propagates and never blocks discovery for other servers.
    /// Pagination is ignored — v1 takes the first page only.
    pub(crate) fn discover_tools(&mut self, server_name: &str) -> Vec<McpTool> {
        match self.call(server_name, "tools/list", serde_json::json!({})) {
            Ok(result) => {
                let tools = result["result"]["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                tools
                    .iter()
                    .filter_map(|t| {
                        let name = t["name"].as_str()?; // skip if name missing
                        let description = t["description"].as_str().unwrap_or("").to_string();
                        Some(McpTool {
                            name: namespaced_tool_name(server_name, name),
                            description,
                            server_name: server_name.to_string(),
                        })
                    })
                    .collect()
            }
            Err(_) => vec![], // per-server error isolation — silent, never propagates
        }
    }

    /// Discovers tools across all configured servers. One server's discovery failure
    /// never blocks the others (each `discover_tools` swallows its own errors).
    pub(crate) fn discover_all(&mut self) -> Vec<McpTool> {
        let names: Vec<String> = self.server_names().map(|s| s.to_string()).collect();
        names
            .iter()
            .flat_map(|name| self.discover_tools(name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::mcp::types::McpServerConfig;

    fn make_config(servers: Vec<McpServerConfig>) -> McpConfig {
        McpConfig { servers }
    }

    #[test]
    fn mcp_config_load_returns_empty_when_no_files_exist() {
        let config = McpConfig::load(None, None);
        assert!(config.servers.is_empty());
    }

    #[test]
    fn mcp_config_merge_project_overrides_home() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut home_file = NamedTempFile::new().unwrap();
        write!(
            home_file,
            r#"{{"servers": [{{"name": "A", "command": "home_cmd", "args": []}}]}}"#
        )
        .unwrap();

        let mut project_file = NamedTempFile::new().unwrap();
        write!(
            project_file,
            r#"{{"servers": [{{"name": "A", "command": "proj_cmd", "args": []}}, {{"name": "B", "command": "b_cmd", "args": []}}]}}"#
        )
        .unwrap();

        let config = McpConfig::load(Some(home_file.path()), Some(project_file.path()));
        assert_eq!(config.servers.len(), 2);

        let a = config.servers.iter().find(|s| s.name == "A").unwrap();
        assert_eq!(
            a.command, "proj_cmd",
            "project should override home for server A"
        );

        assert!(
            config.servers.iter().any(|s| s.name == "B"),
            "server B from project should be present"
        );
    }

    #[test]
    fn mcpmanager_start_all_handles_bad_command_gracefully() {
        let config = make_config(vec![McpServerConfig {
            name: "bad".to_string(),
            command: "nonexistent_binary_xyz_thunk_test".to_string(),
            args: vec![],
            env: None,
        }]);
        let mut manager = MCPManager::new(config);
        manager.start_all(); // must not panic
        assert!(
            manager.sessions.is_empty(),
            "failed server should not appear in sessions map"
        );
    }

    #[test]
    fn discover_tools_returns_namespaced_names() {
        // No live session for "test_server" — discovery must fail silently (no panic) and
        // return an empty vec rather than propagating the error.
        let config = make_config(vec![McpServerConfig {
            name: "test_server".to_string(),
            command: "nonexistent_binary_xyz_thunk_test".to_string(),
            args: vec![],
            env: None,
        }]);
        let mut manager = MCPManager::new(config);
        let tools = manager.discover_tools("test_server");
        assert!(
            tools.is_empty(),
            "discovery against a dead/absent server must return empty, not panic"
        );
    }

    #[test]
    fn discover_all_empty_when_no_servers() {
        let mut manager = MCPManager::new(make_config(vec![]));
        assert!(
            manager.discover_all().is_empty(),
            "no configured servers means no discovered tools"
        );
    }

    #[test]
    fn namespaced_tool_name_uses_mcp_double_colon_convention() {
        // A raw tool "list_files" on server "filesystem" becomes mcp::filesystem::list_files.
        assert_eq!(
            namespaced_tool_name("filesystem", "list_files"),
            "mcp::filesystem::list_files"
        );
    }

    #[test]
    fn call_tool_builds_correct_request_params() {
        // tools/call params must use the BARE tool name and an `arguments` object.
        let args = serde_json::json!({ "path": "/tmp", "depth": 2 });
        let params = build_call_params("list_files", args.clone());
        assert_eq!(params["name"], serde_json::json!("list_files"));
        assert_eq!(params["arguments"], args);
    }

    #[test]
    fn parse_call_result_flattens_text_blocks_and_reads_error_flag() {
        // Multiple text blocks concatenate with newlines; non-text blocks are skipped.
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [
                    { "type": "text", "text": "line one" },
                    { "type": "image", "data": "ignored" },
                    { "type": "text", "text": "line two" }
                ],
                "isError": false
            }
        });
        let result = parse_call_result(&response);
        assert_eq!(result.content, "line one\nline two");
        assert!(!result.is_error);

        // isError: true is preserved (surfaced to the model, not raised as Err).
        let err_response = serde_json::json!({
            "result": { "content": [{ "type": "text", "text": "boom" }], "isError": true }
        });
        let err_result = parse_call_result(&err_response);
        assert_eq!(err_result.content, "boom");
        assert!(err_result.is_error);
    }
}
