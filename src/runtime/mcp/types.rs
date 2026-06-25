use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::core::error::AppError;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl McpConfig {
    /// Loads MCP config from up to two sources and merges them.
    /// home_config is loaded first; per-project entries append and override by server name.
    /// Advisory — returns McpConfig::default() on any parse failure.
    pub(crate) fn load(home_config: Option<&Path>, project_config: Option<&Path>) -> Self {
        let mut merged: Vec<McpServerConfig> = Vec::new();

        for path in [home_config, project_config].into_iter().flatten() {
            if !path.exists() {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<McpConfig>(&raw).or_else(|_| {
                // Also try TOML format
                toml::from_str::<McpConfig>(&raw).map_err(|e| AppError::Config(e.to_string()))
            }) else {
                continue;
            };
            for server in parsed.servers {
                // Per-project (later) overrides home (earlier) by name.
                merged.retain(|s: &McpServerConfig| s.name != server.name);
                merged.push(server);
            }
        }

        McpConfig { servers: merged }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct McpTool {
    pub name: String,
    pub description: String,
    pub server_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct McpCallResult {
    pub content: String,
    pub is_error: bool,
}
