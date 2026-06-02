use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::runtime::ResolvedToolInput;

use super::types::{
    ExecutionKind, GitBranchOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use super::Tool;

const MAX_GIT_BRANCH_STDOUT_BYTES: usize = 16 * 1024;
const MAX_GIT_BRANCH_STDERR_BYTES: usize = 4 * 1024;

pub struct GitBranchTool {
    root: PathBuf,
}

impl GitBranchTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn run_branch(&self) -> Result<ToolRunResult, ToolError> {
        let current = self.run_current_branch()?;
        let branches = self.run_all_branches()?;
        Ok(ToolRunResult::Immediate(ToolOutput::GitBranch(
            GitBranchOutput { current, branches },
        )))
    }

    fn run_current_branch(&self) -> Result<String, ToolError> {
        let output = run_bounded_git_command(
            &self.root,
            &["branch", "--show-current"],
            MAX_GIT_BRANCH_STDOUT_BYTES,
            MAX_GIT_BRANCH_STDERR_BYTES,
        )?;
        if !output.status.success() {
            return Err(git_branch_error(&output.stderr.bytes));
        }
        let stdout = String::from_utf8_lossy(&output.stdout.bytes);
        Ok(stdout.trim().to_string())
    }

    fn run_all_branches(&self) -> Result<Vec<String>, ToolError> {
        let output = run_bounded_git_command(
            &self.root,
            &["branch"],
            MAX_GIT_BRANCH_STDOUT_BYTES,
            MAX_GIT_BRANCH_STDERR_BYTES,
        )?;
        if !output.status.success() {
            return Err(git_branch_error(&output.stderr.bytes));
        }
        let stdout = String::from_utf8_lossy(&output.stdout.bytes);
        Ok(parse_branch_list(&stdout))
    }
}

impl Tool for GitBranchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_branch",
            description: "Show read-only local git branch list and current branch for the project.",
            input_hint: "",
            execution_kind: ExecutionKind::Immediate,
            default_risk: None,
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::GitBranch = input else {
            return Err(ToolError::InvalidInput(
                "git_branch received wrong input variant".into(),
            ));
        };
        self.run_branch()
    }
}

struct BoundedGitOutput {
    status: ExitStatus,
    stdout: BoundedCapture,
    stderr: BoundedCapture,
}

struct BoundedCapture {
    bytes: Vec<u8>,
    _truncated: bool,
}

fn run_bounded_git_command(
    root: &std::path::Path,
    args: &[&str],
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<BoundedGitOutput, ToolError> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(git_command_error)?;

    let stdout = child.stdout.take().ok_or_else(output_capture_error)?;
    let stderr = child.stderr.take().ok_or_else(output_capture_error)?;

    let stdout_reader = thread::spawn(move || read_bounded_stream(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_bounded_stream(stderr, stderr_limit));

    let status = child.wait()?;
    let stdout = join_capture(stdout_reader)?;
    let stderr = join_capture(stderr_reader)?;

    Ok(BoundedGitOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded_stream<R: Read>(mut reader: R, limit: usize) -> io::Result<BoundedCapture> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }

        let remaining = limit.saturating_sub(bytes.len());
        if remaining > 0 {
            let keep = remaining.min(n);
            bytes.extend_from_slice(&buf[..keep]);
        }

        if n > remaining {
            truncated = true;
            break;
        }
    }

    if truncated {
        io::copy(&mut reader, &mut io::sink())?;
    }

    Ok(BoundedCapture {
        bytes,
        _truncated: truncated,
    })
}

fn join_capture(
    handle: thread::JoinHandle<io::Result<BoundedCapture>>,
) -> Result<BoundedCapture, ToolError> {
    handle
        .join()
        .map_err(|_| output_capture_error())?
        .map_err(ToolError::Io)
}

fn output_capture_error() -> ToolError {
    ToolError::InvalidInput("git_branch failed: output capture failed".into())
}

fn git_command_error(error: io::Error) -> ToolError {
    if error.kind() == io::ErrorKind::NotFound {
        ToolError::InvalidInput("git_branch failed: git executable unavailable".into())
    } else {
        ToolError::Io(error)
    }
}

fn git_branch_error(stderr: &[u8]) -> ToolError {
    let stderr = String::from_utf8_lossy(stderr);
    if stderr.to_ascii_lowercase().contains("not a git repository") {
        ToolError::InvalidInput("git_branch failed: not a Git repository".into())
    } else {
        ToolError::InvalidInput("git_branch failed".into())
    }
}

fn parse_branch_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let stripped = line
                .strip_prefix("* ")
                .or_else(|| line.strip_prefix("  "))?;
            let name = stripped.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use tempfile::TempDir;

    use super::*;

    fn init_git_repo(path: &Path) {
        let status = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git init must succeed");
    }

    fn git(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git command must succeed: {args:?}");
    }

    fn commit_file(path: &Path, file: &str, contents: &str) {
        fs::write(path.join(file), contents).unwrap();
        git(path, &["add", file]);
        git(
            path,
            &[
                "-c",
                "user.name=thunk",
                "-c",
                "user.email=thunk@example.invalid",
                "commit",
                "-m",
                "test commit",
            ],
        );
    }

    fn run_branch(path: &Path) -> Result<ToolRunResult, ToolError> {
        GitBranchTool::new(PathBuf::from(path)).run(&ResolvedToolInput::GitBranch)
    }

    #[test]
    fn spec_is_immediate() {
        let tool = GitBranchTool::new(PathBuf::from("."));
        let spec = tool.spec();
        assert_eq!(spec.name, "git_branch");
        assert_eq!(spec.execution_kind, ExecutionKind::Immediate);
        assert!(spec.default_risk.is_none());
    }

    #[test]
    fn non_git_directory_returns_error() {
        let tmp = TempDir::new().unwrap();
        let err = run_branch(tmp.path()).unwrap_err();
        assert!(matches!(
            err,
            ToolError::InvalidInput(ref message)
                if message == "git_branch failed: not a Git repository"
        ));
    }

    #[test]
    fn empty_repo_returns_empty_branch_list() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());

        let out = run_branch(tmp.path()).unwrap();
        let ToolRunResult::Immediate(ToolOutput::GitBranch(branch)) = out else {
            panic!("expected Immediate(GitBranch)");
        };
        assert!(branch.branches.is_empty());
    }

    #[test]
    fn repo_with_commit_returns_current_branch_and_list() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "first.txt", "first\n");

        let out = run_branch(tmp.path()).unwrap();
        let ToolRunResult::Immediate(ToolOutput::GitBranch(branch)) = out else {
            panic!("expected Immediate(GitBranch)");
        };
        assert!(!branch.current.is_empty());
        assert!(!branch.branches.is_empty());
        assert!(branch.branches.contains(&branch.current));
    }

    #[test]
    fn parse_branch_list_strips_prefix() {
        let stdout = "* main\n  feature\n  fix/thing\n";
        let branches = parse_branch_list(stdout);
        assert_eq!(branches, vec!["main", "feature", "fix/thing"]);
    }
}
