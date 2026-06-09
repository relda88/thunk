mod conversation;
pub(crate) mod index;
mod investigation;
pub(crate) mod lsp;
mod orchestration;
mod paths;
pub(crate) mod project;
mod protocol;
#[cfg(test)]
mod tests;
mod trace;
mod types;

pub use crate::tools::{PendingAction, RiskLevel};
pub(crate) use index::{
    extract_symbols, ExtractedSymbol, ImportEdge, SymbolConfidence, SymbolKind,
};
pub use orchestration::Runtime;
pub use project::ResolvedToolInput;
#[allow(unused_imports)]
pub use project::{resolve, PathResolutionError};
pub use project::{ProjectPath, ProjectScope};
pub use project::{ProjectRoot, ProjectRootError};
pub use types::{AnswerSource, DiffMode, RuntimeEvent, RuntimeRequest};
