use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::runtime::ResolvedToolInput;

use crate::tools::pending::{PendingAction, RiskLevel};
use crate::tools::types::{
    ExecutionKind, GitBranchSwitchOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use crate::tools::Tool;

const MAX_GIT_BRANCH_SWITCH_STDOUT_BYTES: usize = 16 * 1024;
const MAX_GIT_BRANCH_SWITCH_STDERR_BYTES: usize = 4 * 1024;

pub struct GitBranchSwitchTool {
    root: PathBuf,
}

impl GitBranchSwitchTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Tool for GitBranchSwitchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_branch_switch",
            description:
                "Switch to an existing local git branch. Requires approval before executing.",
            input_hint: "",
            execution_kind: ExecutionKind::RequiresApproval,
            default_risk: Some(RiskLevel::Medium),
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::GitBranchSwitch { name } = input else {
            return Err(ToolError::InvalidInput(
                "git_branch_switch received wrong input variant".into(),
            ));
        };

        if name.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "git_branch_switch: branch name must not be empty".into(),
            ));
        }

        Ok(ToolRunResult::Approval(PendingAction {
            tool_name: "git_branch_switch".to_string(),
            summary: format!("switch to branch {name}"),
            risk: RiskLevel::Medium,
            payload: name.clone(),
        }))
    }

    fn execute_approved(&self, payload: &str) -> Result<ToolOutput, ToolError> {
        let name = payload;
        if name.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "git_branch_switch: branch name must not be empty".into(),
            ));
        }

        // Revalidation: verify branch exists before switching.
        let list_output = run_bounded_git_command(
            &self.root,
            &["branch", "--list", name],
            MAX_GIT_BRANCH_SWITCH_STDOUT_BYTES,
            MAX_GIT_BRANCH_SWITCH_STDERR_BYTES,
        )?;
        let list_stdout = String::from_utf8_lossy(&list_output.stdout.bytes);
        if list_stdout.trim().is_empty() {
            return Err(ToolError::InvalidInput(format!(
                "git_branch_switch failed: branch '{name}' not found"
            )));
        }

        // Capture current branch before switching for the output record.
        let current_output = run_bounded_git_command(
            &self.root,
            &["branch", "--show-current"],
            MAX_GIT_BRANCH_SWITCH_STDOUT_BYTES,
            MAX_GIT_BRANCH_SWITCH_STDERR_BYTES,
        )?;
        let from = String::from_utf8_lossy(&current_output.stdout.bytes)
            .trim()
            .to_string();

        // Use git checkout for Git < 2.23 compatibility.
        let checkout_output = run_bounded_git_command(
            &self.root,
            &["checkout", name],
            MAX_GIT_BRANCH_SWITCH_STDOUT_BYTES,
            MAX_GIT_BRANCH_SWITCH_STDERR_BYTES,
        )?;
        if !checkout_output.status.success() {
            let stderr = String::from_utf8_lossy(&checkout_output.stderr.bytes);
            return Err(ToolError::InvalidInput(format!(
                "git checkout failed: {stderr}"
            )));
        }

        Ok(ToolOutput::GitBranchSwitch(GitBranchSwitchOutput {
            from,
            to: name.to_string(),
        }))
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
    ToolError::InvalidInput("git_branch_switch failed: output capture failed".into())
}

fn git_command_error(error: io::Error) -> ToolError {
    if error.kind() == io::ErrorKind::NotFound {
        ToolError::InvalidInput("git_branch_switch failed: git executable unavailable".into())
    } else {
        ToolError::Io(error)
    }
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

    fn current_branch(path: &Path) -> String {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn approve_switch(path: &Path, name: &str) -> Result<ToolOutput, ToolError> {
        let tool = GitBranchSwitchTool::new(PathBuf::from(path));
        let result = tool.run(&ResolvedToolInput::GitBranchSwitch {
            name: name.to_string(),
        })?;
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected Approval");
        };
        tool.execute_approved(&pending.payload)
    }

    #[test]
    fn spec_requires_approval() {
        let tool = GitBranchSwitchTool::new(PathBuf::from("."));
        let spec = tool.spec();
        assert_eq!(spec.name, "git_branch_switch");
        assert_eq!(spec.execution_kind, ExecutionKind::RequiresApproval);
        assert_eq!(spec.default_risk, Some(RiskLevel::Medium));
    }

    #[test]
    fn run_returns_approval_with_correct_fields() {
        let result = GitBranchSwitchTool::new(PathBuf::from("."))
            .run(&ResolvedToolInput::GitBranchSwitch {
                name: "main".to_string(),
            })
            .unwrap();
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected Approval");
        };
        assert_eq!(pending.tool_name, "git_branch_switch");
        assert_eq!(pending.risk, RiskLevel::Medium);
        assert!(pending.summary.contains("main"));
        assert_eq!(pending.payload, "main");
    }

    #[test]
    fn switches_branch() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "a\n");
        git(tmp.path(), &["branch", "feature"]);

        let starting = current_branch(tmp.path());
        let out = approve_switch(tmp.path(), "feature").unwrap();
        let ToolOutput::GitBranchSwitch(o) = out else {
            panic!("expected GitBranchSwitch");
        };
        assert_eq!(o.from, starting);
        assert_eq!(o.to, "feature");
        assert_eq!(current_branch(tmp.path()), "feature");
    }

    #[test]
    fn rejects_missing_branch() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "a\n");

        let err = approve_switch(tmp.path(), "nonexistent").unwrap_err();
        assert!(matches!(
            err,
            ToolError::InvalidInput(ref msg) if msg.contains("not found")
        ));
    }

    #[test]
    fn rejects_empty_name() {
        let result =
            GitBranchSwitchTool::new(PathBuf::from(".")).run(&ResolvedToolInput::GitBranchSwitch {
                name: "".to_string(),
            });
        assert!(matches!(result.unwrap_err(), ToolError::InvalidInput(_)));
    }
}
