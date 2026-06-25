use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::core::error::Result;

use super::session::McpSession;
use super::types::{McpConfig, McpServerConfig};

pub(crate) struct MCPManager {
    sessions: HashMap<String, McpSession>,
    config: McpConfig,
    timeout: Duration,
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
}
