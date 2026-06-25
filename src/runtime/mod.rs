mod conversation;
pub(crate) mod diagnostics;
pub(crate) mod diff;
pub(crate) mod index;
mod investigation;
pub(crate) mod lsp;
pub(crate) mod mcp;
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
pub use orchestration::Runtime;
pub use project::ProjectRoot;
pub use project::ResolvedToolInput;
#[allow(unused_imports)]
pub use project::{resolve, PathResolutionError};
pub use project::{ProjectPath, ProjectScope};
pub use types::{AnswerSource, DiffMode, RuntimeEvent, RuntimeRequest};
