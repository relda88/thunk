mod manager;
mod session;
mod transport;
mod types;

pub(crate) use manager::MCPManager;
pub(crate) use types::McpConfig;
// McpServerConfig, McpTool, McpCallResult are used in Slice 45.3+.
#[allow(unused_imports)]
pub(crate) use types::{McpCallResult, McpServerConfig, McpTool};
