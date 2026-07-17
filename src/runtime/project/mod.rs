mod cargo_context;
mod project_path;
mod project_root;
mod project_snapshot;
mod resolved_input;
mod resolver;

pub use cargo_context::CargoContext;
pub use project_path::{ProjectPath, ProjectScope};
pub use project_root::{ProjectRoot, ProjectRootError};
pub(crate) use project_snapshot::{
    ProjectStructureEntry, ProjectStructureEntryKind, ProjectStructureSnapshot,
    ProjectStructureSnapshotCache, MAX_SNAPSHOT_NODES,
};
pub use resolved_input::ResolvedToolInput;
pub(crate) use resolver::check_shell_command_scope;
#[allow(unused_imports)]
pub use resolver::{resolve, PathResolutionError};
