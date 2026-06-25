pub mod core;
pub mod git;
mod pending;
mod registry;
pub mod search;
pub mod types;

use crate::runtime::ResolvedToolInput;

use core::ListDirTool;
use core::ReadFileTool;

pub use git::GitDiffTool;
pub use pending::{PendingAction, PendingApprovalStage, PendingTransaction, RiskLevel};
pub use registry::ToolRegistry;
pub use types::{
    DynamicToolSpec, EntryKind, ExecutionKind, ToolError, ToolInput, ToolOutput, ToolRunResult,
    ToolSpec,
};

/// The core tool trait. Each implementation handles exactly one ToolInput variant.
///
/// Read-only tools implement only run(). Mutating tools implement both run() and
/// execute_approved(). The two-phase design ensures mutations never happen without
/// explicit approval.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    /// Phase 1 of execution: validate input and return either an immediate result
    /// or a PendingAction describing the proposed mutation.
    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError>;

    /// Phase 2 of execution: apply a previously approved mutation and return the
    /// result. Only mutating tools implement this — read-only tools never produce
    /// Approval outcomes and this method is never called on them.
    fn execute_approved(&self, _payload: &str) -> Result<ToolOutput, ToolError> {
        Err(ToolError::InvalidInput(format!(
            "tool '{}' does not support approved execution",
            self.spec().name
        )))
    }
}

/// Builds a ToolRegistry with the tools that do not require a project root.
///
/// Call `ToolRegistry::with_project_root()` to add the root-aware tools that
/// need the runtime-owned project root for execution or approval validation.
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::new());
    registry.register(ListDirTool::new());
    registry
}
