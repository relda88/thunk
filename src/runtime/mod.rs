mod conversation;
pub(crate) mod diagnostics;
pub(crate) mod diff;
pub(crate) mod index;
mod investigation;
pub(crate) mod lsp;
pub(crate) mod mcp;
pub(crate) mod memory;
mod orchestration;
mod patch;
mod paths;
pub(crate) mod project;
mod protocol;
#[cfg(test)]
mod tests;
mod trace;
mod types;

pub(crate) use index::{SymbolConfidence, SymbolKind};
pub(crate) use investigation::shell_tier::{classify_shell_tier, ShellTier};
pub use orchestration::Runtime;
pub(crate) use project::check_shell_command_scope;
pub use project::ProjectRoot;
pub use project::ResolvedToolInput;
#[allow(unused_imports)]
pub use project::{resolve, PathResolutionError};
pub use project::{ProjectPath, ProjectScope};
pub use types::{
    Activity, AnswerSource, DiffMode, RuntimeEvent, RuntimeRequest, RuntimeTerminalReason,
};
