#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::dirs::DEFAULT_SKIP_DIRS;
use crate::tools::{ToolError, ToolInput};

use super::{
    project_path::relative_display, ProjectPath, ProjectRoot, ProjectScope, ResolvedToolInput,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PathResolutionError {
    #[error("path '{raw}' escapes project root {}", root.display())]
    EscapesRoot { raw: String, root: PathBuf },

    #[error("path not found: '{raw}'")]
    NotFound { raw: String },

    #[error("path is not a directory: '{raw}'")]
    NotADirectory { raw: String },

    #[error("path '{raw}' uses symlink parent '{component}'")]
    SymlinkParent { raw: String, component: String },

    #[error("path '{raw}' resolves to symlink target {}", target.display())]
    SymlinkTarget { raw: String, target: PathBuf },

    #[error("invalid path '{raw}': {reason}")]
    InvalidPath { raw: String, reason: String },
}

impl From<PathResolutionError> for ToolError {
    fn from(error: PathResolutionError) -> Self {
        match error {
            PathResolutionError::EscapesRoot { raw, root } => ToolError::InvalidInput(format!(
                "path escapes project root: '{raw}' is outside {} — for paths outside the project, use [mcp::filesystem::list_directory: /full/path] or other mcp::filesystem tools instead.",
                root.display()
            )),
            PathResolutionError::NotFound { raw } => {
                ToolError::InvalidInput(format!("path not found: '{raw}'"))
            }
            PathResolutionError::NotADirectory { raw } => {
                ToolError::InvalidInput(format!("path is not a directory: '{raw}'"))
            }
            PathResolutionError::SymlinkParent { raw, component } => ToolError::InvalidInput(
                format!("path uses symlink parent: '{raw}' via '{component}'"),
            ),
            PathResolutionError::SymlinkTarget { raw, target } => ToolError::InvalidInput(format!(
                "path resolves to symlink target: '{raw}' -> {}",
                target.display()
            )),
            PathResolutionError::InvalidPath { raw, reason } => {
                ToolError::InvalidInput(format!("invalid path: '{raw}': {reason}"))
            }
        }
    }
}

pub fn resolve(
    root: &ProjectRoot,
    input: &ToolInput,
) -> Result<ResolvedToolInput, PathResolutionError> {
    match input {
        ToolInput::ReadFile { path } => Ok(ResolvedToolInput::ReadFile {
            path: resolve_read_path(root, path)?,
        }),
        ToolInput::ListDir { path } => Ok(ResolvedToolInput::ListDir {
            path: resolve_scope(root, path)?,
        }),
        ToolInput::SearchCode { query, path } => Ok(ResolvedToolInput::SearchCode {
            query: query.clone(),
            scope: path
                .as_deref()
                .map(|raw| resolve_scope(root, raw))
                .transpose()?,
        }),
        ToolInput::WriteFile { path, content } => Ok(ResolvedToolInput::WriteFile {
            path: resolve_write_path(root, path)?,
            content: content.clone(),
        }),
        ToolInput::EditFile {
            path,
            search,
            replace,
        } => Ok(ResolvedToolInput::EditFile {
            path: resolve_write_path(root, path)?,
            search: search.clone(),
            replace: replace.clone(),
        }),
        ToolInput::Shell { command } => {
            check_shell_command_scope(root.path(), command)?;
            Ok(ResolvedToolInput::Shell {
                command: command.clone(),
            })
        }
        ToolInput::ShellRead { command } => {
            check_shell_command_scope(root.path(), command)?;
            Ok(ResolvedToolInput::ShellRead {
                command: command.clone(),
            })
        }
        ToolInput::GitStatus => Ok(ResolvedToolInput::GitStatus),
        ToolInput::GitDiff => Ok(ResolvedToolInput::GitDiff { path: None }),
        ToolInput::GitLog => Ok(ResolvedToolInput::GitLog),
        ToolInput::GitBranch => Ok(ResolvedToolInput::GitBranch),
        ToolInput::GitBranchCreate { name, start_point } => {
            Ok(ResolvedToolInput::GitBranchCreate {
                name: name.clone(),
                start_point: start_point.clone(),
            })
        }
        ToolInput::GitBranchSwitch { name } => {
            Ok(ResolvedToolInput::GitBranchSwitch { name: name.clone() })
        }
        ToolInput::GitCommit { message } => Ok(ResolvedToolInput::GitCommit {
            message: message.clone(),
        }),
        ToolInput::GitDiffStaged => Ok(ResolvedToolInput::GitDiffStaged),
        ToolInput::LspDefinition { path, line, col } => {
            let resolved = resolve_read_path(root, path)?;
            Ok(ResolvedToolInput::LspDefinition {
                path: resolved.absolute().to_string_lossy().into_owned(),
                line: *line,
                col: *col,
            })
        }
        ToolInput::WebFetch { url } => Ok(ResolvedToolInput::WebFetch { url: url.clone() }),
        // Dynamic tools bypass path confinement — they validate their own inputs.
        // Path resolution is MCP-server responsibility, not the runtime's.
        ToolInput::DynamicTool { name, args } => Ok(ResolvedToolInput::DynamicTool {
            name: name.clone(),
            args: args.clone(),
        }),
    }
}

/// Rejects a shell command whose argument tokens resolve to real paths outside the
/// project root.
///
/// The shell tools spawn their program directly with the project root as cwd, but pass
/// every token after the program name through verbatim. Without this check a Tier-1
/// `cat /etc/passwd` reaches further than `read_file` ever can.
///
/// Each token except the program name is resolved strictly relative to the project
/// root, with three outcomes:
/// - resolves inside the root — permitted
/// - resolves to nothing — permitted; flags (`-n`), sed scripts (`s/foo/bar/p`), and
///   search patterns name no file, so there is nothing to disclose
/// - resolves to a real path outside the root — `EscapesRoot`, rejecting the whole
///   command before any part of it is spawned
///
/// Token 0 is skipped: an absolute program path (`/usr/bin/cat`) is legitimate and is
/// not an operand. `classify_shell_tier` routes metacharacters to Tier 3 before any
/// tool sees them, so tokens here are always literal and unexpanded.
pub(crate) fn check_shell_command_scope(
    root: &Path,
    command: &str,
) -> Result<(), PathResolutionError> {
    for token in command.split_whitespace().skip(1) {
        check_shell_token_scope(root, token)?;
    }
    Ok(())
}

/// Resolves one shell argument token against the project root.
///
/// Deliberately narrower than `resolve_read_path`: it does not fall back to
/// `find_unique_file_in_project`. The spawned process interprets its operands relative
/// to its cwd, so a project-wide filename walk would validate a different file than the
/// one actually opened — `sed -i s/foo/bar/ file.txt` would be checked against whichever
/// `file.txt` the walk happened to find. Confinement must check the path that will be
/// used, not a same-named path elsewhere in the tree.
fn check_shell_token_scope(root: &Path, raw: &str) -> Result<(), PathResolutionError> {
    let raw_path = Path::new(raw);
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        root.join(raw_path)
    };

    // Canonicalize resolves `..` and symlinks, so both relative escapes and symlink
    // escapes land on their real target. A token that names nothing on disk cannot
    // disclose anything, so a canonicalize failure is permitted rather than rejected.
    let Ok(canonical) = fs::canonicalize(&candidate) else {
        return Ok(());
    };

    #[cfg(target_os = "windows")]
    let canonical = {
        let s = canonical.to_string_lossy();
        if s.starts_with("\\\\?\\") {
            std::path::PathBuf::from(&s[4..])
        } else {
            canonical
        }
    };

    if relative_display(&canonical, root).is_none() {
        return Err(PathResolutionError::EscapesRoot {
            raw: raw.to_string(),
            root: root.to_path_buf(),
        });
    }

    Ok(())
}

const MAX_FILENAME_SEARCH_NODES: usize = 500;

/// Walks the project tree looking for a file whose name matches `filename`.
///
/// Uses a depth-first stack walk capped at `MAX_FILENAME_SEARCH_NODES` entries.
/// Skips `DEFAULT_SKIP_DIRS` at every level. Returns `None` when zero matches
/// are found, when more than one match is found (ambiguous), or when the node
/// budget is exhausted before the walk completes.
fn find_unique_file_in_project(root: &Path, filename: &str) -> Option<PathBuf> {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut found: Option<PathBuf> = None;
    let mut nodes = 0usize;

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if nodes >= MAX_FILENAME_SEARCH_NODES {
                return None;
            }
            nodes += 1;

            let path = entry.path();
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };

            if path.is_dir() {
                if DEFAULT_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if name == filename {
                if found.is_some() {
                    return None; // ambiguous
                }
                found = Some(path);
            }
        }
    }

    found
}

fn resolve_read_path(root: &ProjectRoot, raw: &str) -> Result<ProjectPath, PathResolutionError> {
    let raw_path = Path::new(raw);
    let candidate = if !raw.contains('/') && !raw.contains('\\') && raw_path.extension().is_some() {
        find_unique_file_in_project(root.path(), raw).ok_or_else(|| {
            PathResolutionError::NotFound {
                raw: raw.to_string(),
            }
        })?
    } else if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        root.path().join(raw_path)
    };

    let canonical = fs::canonicalize(&candidate).map_err(|_| PathResolutionError::NotFound {
        raw: raw.to_string(),
    })?;

    #[cfg(target_os = "windows")]
    let canonical = {
        let s = canonical.to_string_lossy();
        if s.starts_with("\\\\?\\") {
            std::path::PathBuf::from(&s[4..])
        } else {
            canonical
        }
    };

    project_path_from_absolute(root, raw, canonical)
}

fn resolve_write_path(root: &ProjectRoot, raw: &str) -> Result<ProjectPath, PathResolutionError> {
    let normalized = normalize_write_path(root, raw)?;
    let relative =
        normalized
            .strip_prefix(root.path())
            .map_err(|_| PathResolutionError::EscapesRoot {
                raw: raw.to_string(),
                root: root.path().to_path_buf(),
            })?;

    let components = relative_components(relative, raw)?;
    let final_path = rebuild_write_target(root, raw, &components)?;

    if !final_path.starts_with(root.path()) {
        return Err(PathResolutionError::EscapesRoot {
            raw: raw.to_string(),
            root: root.path().to_path_buf(),
        });
    }

    match fs::symlink_metadata(&final_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(PathResolutionError::SymlinkTarget {
                raw: raw.to_string(),
                target: final_path,
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(PathResolutionError::InvalidPath {
                raw: raw.to_string(),
                reason: format!("cannot inspect target {}: {error}", final_path.display()),
            });
        }
    }

    project_path_from_absolute(root, raw, final_path)
}

fn resolve_scope(root: &ProjectRoot, raw: &str) -> Result<ProjectScope, PathResolutionError> {
    let path = resolve_read_path(root, raw)?;
    if path.absolute().is_dir() {
        return Ok(ProjectScope::from_trusted_path(path));
    }
    // raw pointed to a file — use its parent directory as the scope
    let parent = path
        .absolute()
        .parent()
        .ok_or_else(|| PathResolutionError::NotADirectory {
            raw: raw.to_string(),
        })?;
    let parent_path = project_path_from_absolute(root, raw, parent.to_path_buf())?;
    Ok(ProjectScope::from_trusted_path(parent_path))
}

fn project_path_from_absolute(
    root: &ProjectRoot,
    raw: &str,
    absolute: PathBuf,
) -> Result<ProjectPath, PathResolutionError> {
    let relative = relative_display(&absolute, root.path()).ok_or_else(|| {
        PathResolutionError::EscapesRoot {
            raw: raw.to_string(),
            root: root.path().to_path_buf(),
        }
    })?;

    Ok(ProjectPath::from_trusted(absolute, relative))
}

fn normalize_write_path(root: &ProjectRoot, raw: &str) -> Result<PathBuf, PathResolutionError> {
    let raw_path = Path::new(raw);
    if raw_path.is_absolute() {
        normalize_absolute_path(raw_path, raw)
    } else {
        normalize_relative_path(root, raw_path, raw)
    }
}

fn normalize_relative_path(
    root: &ProjectRoot,
    raw_path: &Path,
    raw: &str,
) -> Result<PathBuf, PathResolutionError> {
    let mut normalized = root.path().to_path_buf();
    let boundary = root.path().components().count();

    for component in raw_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if normalized.components().count() == boundary {
                    return Err(PathResolutionError::EscapesRoot {
                        raw: raw.to_string(),
                        root: root.path().to_path_buf(),
                    });
                }
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(PathResolutionError::InvalidPath {
                    raw: raw.to_string(),
                    reason: "unexpected absolute component in relative path".to_string(),
                });
            }
        }
    }

    if !normalized.starts_with(root.path()) {
        return Err(PathResolutionError::EscapesRoot {
            raw: raw.to_string(),
            root: root.path().to_path_buf(),
        });
    }

    Ok(normalized)
}

fn normalize_absolute_path(path: &Path, raw: &str) -> Result<PathBuf, PathResolutionError> {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(PathResolutionError::InvalidPath {
                        raw: raw.to_string(),
                        reason: "path traverses above filesystem root".to_string(),
                    });
                }
            }
        }
    }

    Ok(normalized)
}

fn relative_components(relative: &Path, raw: &str) -> Result<Vec<OsString>, PathResolutionError> {
    let mut components = Vec::new();

    for component in relative.components() {
        match component {
            Component::Normal(part) => components.push(part.to_os_string()),
            Component::CurDir => {}
            other => {
                return Err(PathResolutionError::InvalidPath {
                    raw: raw.to_string(),
                    reason: format!(
                        "unexpected normalized component: {}",
                        other.as_os_str().to_string_lossy()
                    ),
                });
            }
        }
    }

    Ok(components)
}

fn rebuild_write_target(
    root: &ProjectRoot,
    raw: &str,
    components: &[OsString],
) -> Result<PathBuf, PathResolutionError> {
    if components.is_empty() {
        return Ok(root.path().to_path_buf());
    }

    let parent_component_count = components.len().saturating_sub(1);
    let mut current = root.path().to_path_buf();
    let mut first_missing_parent = parent_component_count;

    for (index, component) in components.iter().take(parent_component_count).enumerate() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                let display = relative_display(&current, root.path())
                    .unwrap_or_else(|| component.to_string_lossy().into_owned());

                if metadata.file_type().is_symlink() {
                    return Err(PathResolutionError::SymlinkParent {
                        raw: raw.to_string(),
                        component: display,
                    });
                }

                if !metadata.is_dir() {
                    return Err(PathResolutionError::InvalidPath {
                        raw: raw.to_string(),
                        reason: format!("parent is not a directory: {display}"),
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current.pop();
                first_missing_parent = index;
                break;
            }
            Err(error) => {
                return Err(PathResolutionError::InvalidPath {
                    raw: raw.to_string(),
                    reason: format!("cannot inspect parent {}: {error}", current.display()),
                });
            }
        }
    }

    let canonical_parent =
        fs::canonicalize(&current).map_err(|error| PathResolutionError::InvalidPath {
            raw: raw.to_string(),
            reason: format!(
                "cannot canonicalize existing parent {}: {error}",
                current.display()
            ),
        })?;

    #[cfg(target_os = "windows")]
    let canonical_parent = {
        let s = canonical_parent.to_string_lossy();
        if s.starts_with("\\\\?\\") {
            std::path::PathBuf::from(&s[4..])
        } else {
            canonical_parent
        }
    };

    if !canonical_parent.starts_with(root.path()) {
        return Err(PathResolutionError::EscapesRoot {
            raw: raw.to_string(),
            root: root.path().to_path_buf(),
        });
    }

    let mut final_path = canonical_parent;
    let remaining_components: Vec<&OsString> = if first_missing_parent < parent_component_count {
        components[first_missing_parent..].iter().collect()
    } else {
        vec![components.last().expect("components is non-empty")]
    };

    for component in remaining_components {
        final_path.push(component);
    }

    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn symlink_file(src: &Path, dst: &Path) {
        std::os::unix::fs::symlink(src, dst).unwrap();
    }

    #[cfg(unix)]
    fn symlink_dir(src: &Path, dst: &Path) {
        std::os::unix::fs::symlink(src, dst).unwrap();
    }

    #[cfg(windows)]
    fn symlink_file(src: &Path, dst: &Path) {
        std::os::windows::fs::symlink_file(src, dst).unwrap();
    }

    #[cfg(windows)]
    fn symlink_dir(src: &Path, dst: &Path) {
        std::os::windows::fs::symlink_dir(src, dst).unwrap();
    }

    fn temp_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    fn make_root() -> (TempDir, ProjectRoot) {
        let dir = temp_dir();
        let root = ProjectRoot::new(dir.path().to_path_buf()).unwrap();
        (dir, root)
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn read_relative_path_inside_root() {
        let (_dir, root) = make_root();
        write_file(&root.path().join("src/main.rs"), "fn main() {}\n");

        let resolved = resolve_read_path(&root, "src/main.rs").unwrap();

        assert_eq!(resolved.absolute(), root.path().join("src/main.rs"));
        assert_eq!(resolved.display(), "src/main.rs");
    }

    #[test]
    fn read_absolute_path_inside_root() {
        let (_dir, root) = make_root();
        let file = root.path().join("README.md");
        write_file(&file, "hello\n");

        let resolved = resolve_read_path(&root, file.to_str().unwrap()).unwrap();

        assert_eq!(resolved.absolute(), file);
        assert_eq!(resolved.display(), "README.md");
    }

    #[test]
    fn read_absolute_path_outside_root_is_rejected() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let outside_file = outside.path().join("outside.txt");
        write_file(&outside_file, "outside\n");
        let raw = outside_file.display().to_string();

        let err = resolve_read_path(&root, &raw).unwrap_err();

        assert!(matches!(
            err,
            PathResolutionError::EscapesRoot { raw: actual, .. } if actual == raw
        ));
    }

    #[test]
    fn read_parent_escape_is_rejected() {
        let (_dir, root) = make_root();
        let outside_file = root.path().parent().unwrap().join("outside.txt");
        write_file(&outside_file, "outside\n");

        let err = resolve_read_path(&root, "../outside.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
        fs::remove_file(outside_file).unwrap();
    }

    #[test]
    fn read_nonexistent_path_is_not_found() {
        let (_dir, root) = make_root();

        let err = resolve_read_path(&root, "missing.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::NotFound { .. }));
    }

    #[test]
    fn read_symlink_pointing_outside_root_is_rejected() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let outside_file = outside.path().join("outside.txt");
        write_file(&outside_file, "outside\n");
        symlink_file(&outside_file, &root.path().join("link.txt"));

        let err = resolve_read_path(&root, "link.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
    }

    #[test]
    fn scope_valid_directory() {
        let (_dir, root) = make_root();
        fs::create_dir_all(root.path().join("src/runtime")).unwrap();

        let scope = resolve_scope(&root, "src").unwrap();

        assert_eq!(scope.absolute(), root.path().join("src"));
        assert_eq!(scope.display(), "src");
    }

    #[test]
    fn scope_file_path_falls_back_to_parent_directory() {
        let (_dir, root) = make_root();
        write_file(&root.path().join("src/lib.rs"), "// lib\n");

        let scope = resolve_scope(&root, "src/lib.rs").unwrap();

        assert_eq!(scope.absolute(), root.path().join("src"));
        assert_eq!(scope.display(), "src");
    }

    #[test]
    fn scope_file_at_root_falls_back_to_root_directory() {
        let (_dir, root) = make_root();
        write_file(&root.path().join("notes.txt"), "notes\n");

        let scope = resolve_scope(&root, "notes.txt").unwrap();

        assert_eq!(scope.absolute(), root.path());
    }

    #[test]
    fn write_new_file_inside_root() {
        let (_dir, root) = make_root();

        let resolved = resolve_write_path(&root, "new.txt").unwrap();

        assert_eq!(resolved.absolute(), root.path().join("new.txt"));
        assert_eq!(resolved.display(), "new.txt");
    }

    #[test]
    fn write_nested_file_inside_root() {
        let (_dir, root) = make_root();
        fs::create_dir_all(root.path().join("src/bin")).unwrap();

        let resolved = resolve_write_path(&root, "src/bin/tool.rs").unwrap();

        assert_eq!(resolved.absolute(), root.path().join("src/bin/tool.rs"));
        assert_eq!(resolved.display(), "src/bin/tool.rs");
    }

    #[test]
    fn write_parent_escape_is_rejected() {
        let (_dir, root) = make_root();

        let err = resolve_write_path(&root, "../escape.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
    }

    #[test]
    fn write_absolute_outside_root_is_rejected() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let raw = outside.path().join("outside.txt").display().to_string();

        let err = resolve_write_path(&root, &raw).unwrap_err();

        assert!(matches!(
            err,
            PathResolutionError::EscapesRoot { raw: actual, .. } if actual == raw
        ));
    }

    #[test]
    fn write_parent_symlink_is_rejected() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        fs::create_dir_all(outside.path().join("real")).unwrap();
        symlink_dir(&outside.path().join("real"), &root.path().join("linked"));

        let err = resolve_write_path(&root, "linked/file.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::SymlinkParent { .. }));
    }

    #[test]
    fn write_existing_target_symlink_is_rejected() {
        let (_dir, root) = make_root();
        let real = root.path().join("real.txt");
        let link = root.path().join("link.txt");
        write_file(&real, "hello\n");
        symlink_file(&real, &link);

        let err = resolve_write_path(&root, "link.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::SymlinkTarget { .. }));
    }

    #[test]
    fn write_existing_real_file_is_allowed() {
        let (_dir, root) = make_root();
        let existing = root.path().join("existing.txt");
        write_file(&existing, "hello\n");

        let resolved = resolve_write_path(&root, "existing.txt").unwrap();

        assert_eq!(resolved.absolute(), existing);
        assert_eq!(resolved.display(), "existing.txt");
    }

    #[test]
    fn write_deep_path_normalization() {
        let (_dir, root) = make_root();

        let resolved = resolve_write_path(&root, "./a/./b/../c/../file.txt").unwrap();

        assert_eq!(resolved.absolute(), root.path().join("a/file.txt"));
        assert_eq!(resolved.display(), "a/file.txt");
    }

    #[test]
    fn path_resolution_error_maps_to_structured_tool_error() {
        let tool_error: crate::tools::ToolError = PathResolutionError::EscapesRoot {
            raw: "../secret.txt".into(),
            root: PathBuf::from("/project"),
        }
        .into();

        assert_eq!(
            tool_error.to_string(),
            "invalid tool input: path escapes project root: '../secret.txt' is outside /project — for paths outside the project, use [mcp::filesystem::list_directory: /full/path] or other mcp::filesystem tools instead."
        );
    }

    // ── shell command scope ──────────────────────────────────────────────────

    fn shell_scope(root: &ProjectRoot, command: &str) -> Result<(), PathResolutionError> {
        check_shell_command_scope(root.path(), command)
    }

    #[test]
    fn shell_absolute_operand_outside_root_is_rejected() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let secret = outside.path().join("passwd");
        write_file(&secret, "root:x:0:0\n");
        let raw = secret.display().to_string();

        let err = shell_scope(&root, &format!("cat {raw}")).unwrap_err();

        assert!(matches!(
            err,
            PathResolutionError::EscapesRoot { raw: actual, .. } if actual == raw
        ));
    }

    #[test]
    fn shell_relative_escape_operand_is_rejected() {
        let (_dir, root) = make_root();
        let outside_file = root.path().parent().unwrap().join("shell_escape.txt");
        write_file(&outside_file, "outside\n");

        let err = shell_scope(&root, "cat ../shell_escape.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
        fs::remove_file(outside_file).unwrap();
    }

    #[test]
    fn shell_symlink_operand_pointing_outside_root_is_rejected() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let outside_file = outside.path().join("outside.txt");
        write_file(&outside_file, "outside\n");
        symlink_file(&outside_file, &root.path().join("link.txt"));

        let err = shell_scope(&root, "cat link.txt").unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
    }

    /// The main risk of a heavy-handed confinement check: operands that merely look
    /// path-shaped but name nothing on disk must stay permitted.
    #[test]
    fn shell_non_path_operands_are_permitted() {
        let (_dir, root) = make_root();
        write_file(&root.path().join("file.txt"), "foo\n");

        // sed script fragment: contains slashes, resolves to nothing.
        shell_scope(&root, "sed -n s/foo/bar/p file.txt").unwrap();
        // grep pattern + flags.
        shell_scope(&root, "grep -rn fn main src/").unwrap();
        // find primaries and a quoted glob that matches no literal path.
        shell_scope(&root, "find . -name '*.rs'").unwrap();
        // wc flags.
        shell_scope(&root, "wc -l file.txt").unwrap();
    }

    #[test]
    fn shell_operand_inside_root_is_permitted() {
        let (_dir, root) = make_root();
        write_file(&root.path().join("src/main.rs"), "fn main() {}\n");

        shell_scope(&root, "cat src/main.rs").unwrap();
        shell_scope(
            &root,
            &format!("cat {}", root.path().join("src/main.rs").display()),
        )
        .unwrap();
    }

    /// Token 0 is the program, not an operand — an absolute program path outside the
    /// root (the normal case for any system binary) must not be rejected.
    #[test]
    fn shell_absolute_program_path_is_not_treated_as_operand() {
        let (_dir, root) = make_root();
        fs::create_dir_all(root.path().join("src")).unwrap();

        shell_scope(&root, "/bin/ls src").unwrap();
    }

    #[test]
    fn shell_commands_with_no_operands_are_permitted() {
        let (_dir, root) = make_root();

        shell_scope(&root, "pwd").unwrap();
        shell_scope(&root, "whoami").unwrap();
    }

    /// Shell argument confinement resolves strictly relative to the root — it must not
    /// inherit resolve_read_path's project-wide bare-filename walk, which would validate
    /// a different file than the spawned process actually opens.
    #[test]
    fn shell_bare_filename_does_not_trigger_project_wide_walk() {
        let (_dir, root) = make_root();
        write_file(&root.path().join("deep/nested/only.txt"), "content\n");

        // `cat only.txt` from the root cwd names root/only.txt, which does not exist.
        // The walk would have found deep/nested/only.txt and validated that instead.
        shell_scope(&root, "cat only.txt").unwrap();

        // Contrast: resolve_read_path does perform the walk for the same raw string.
        let walked = resolve_read_path(&root, "only.txt").unwrap();
        assert_eq!(walked.display(), "deep/nested/only.txt");
    }

    #[test]
    fn resolve_rejects_shell_read_escaping_operand() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let secret = outside.path().join("secret.txt");
        write_file(&secret, "secret\n");

        let err = resolve(
            &root,
            &ToolInput::ShellRead {
                command: format!("cat {}", secret.display()),
            },
        )
        .unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
    }

    /// Tier 2/3 must be confined at resolve() time — before an approval prompt is ever
    /// built, so the user is never asked to approve an out-of-root command.
    #[test]
    fn resolve_rejects_shell_escaping_operand_before_approval() {
        let (_dir, root) = make_root();
        let outside = temp_dir();
        let target = outside.path().join("target.txt");
        write_file(&target, "data\n");

        let err = resolve(
            &root,
            &ToolInput::Shell {
                command: format!("cp {} stolen.txt", target.display()),
            },
        )
        .unwrap_err();

        assert!(matches!(err, PathResolutionError::EscapesRoot { .. }));
    }

    #[test]
    fn resolve_permits_in_root_shell_commands() {
        let (_dir, root) = make_root();
        fs::create_dir_all(root.path().join("src")).unwrap();

        resolve(
            &root,
            &ToolInput::ShellRead {
                command: "ls src".to_string(),
            },
        )
        .unwrap();
        resolve(
            &root,
            &ToolInput::Shell {
                command: "mkdir build".to_string(),
            },
        )
        .unwrap();
    }

    #[test]
    fn bare_filename_resolves_when_unique() {
        let (_dir, root) = make_root();
        write_file(
            &root.path().join("sandbox/services/task_service.py"),
            "def filtered_tasks(tasks): pass\n",
        );

        let resolved = resolve_read_path(&root, "task_service.py").unwrap();

        assert_eq!(
            resolved.absolute(),
            root.path().join("sandbox/services/task_service.py")
        );
        assert_eq!(resolved.display(), "sandbox/services/task_service.py");
    }

    #[test]
    fn bare_filename_returns_not_found_when_ambiguous() {
        let (_dir, root) = make_root();
        write_file(
            &root.path().join("sandbox/services/task_service.py"),
            "# service a\n",
        );
        write_file(
            &root.path().join("sandbox/cli/task_service.py"),
            "# service b\n",
        );

        let err = resolve_read_path(&root, "task_service.py").unwrap_err();

        assert!(
            matches!(err, PathResolutionError::NotFound { .. }),
            "ambiguous bare filename must return NotFound: {err:?}"
        );
    }

    #[test]
    fn bare_filename_skips_default_skip_dirs() {
        let (_dir, root) = make_root();
        // File only exists inside a skip dir — must not be found.
        write_file(
            &root.path().join("target/debug/build_artifact.py"),
            "# should be skipped\n",
        );

        let err = resolve_read_path(&root, "build_artifact.py").unwrap_err();

        assert!(matches!(err, PathResolutionError::NotFound { .. }));
    }
}
