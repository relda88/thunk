use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::runtime::ResolvedToolInput;

use super::pending::{PendingAction, RiskLevel};
use super::types::{
    ExecutionKind, GitCommitOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use super::Tool;

const MAX_GIT_COMMIT_STDOUT_BYTES: usize = 16 * 1024;
const MAX_GIT_COMMIT_STDERR_BYTES: usize = 8 * 1024;

pub struct GitCommitTool {
    root: PathBuf,
}

impl GitCommitTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Tool for GitCommitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_commit",
            description: "Commit currently staged changes with the given message. Does not stage files — commit only what is already staged. Requires approval before executing.",
            input_hint: "",
            execution_kind: ExecutionKind::RequiresApproval,
            default_risk: Some(RiskLevel::High),
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::GitCommit { message } = input else {
            return Err(ToolError::InvalidInput(
                "git_commit received wrong input variant".into(),
            ));
        };

        if message.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "git_commit: commit message must not be empty".into(),
            ));
        }

        let subject = message.lines().next().unwrap_or(message.as_str());
        let display = if subject.len() > 72 {
            format!("{}...", &subject[..69])
        } else {
            subject.to_string()
        };

        Ok(ToolRunResult::Approval(PendingAction {
            tool_name: "git_commit".to_string(),
            summary: format!("commit: {display}"),
            risk: RiskLevel::High,
            // Payload is the raw commit message; decoded verbatim in execute_approved.
            payload: message.clone(),
        }))
    }

    fn execute_approved(&self, payload: &str) -> Result<ToolOutput, ToolError> {
        let message = payload;

        if message.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "git_commit: commit message must not be empty".into(),
            ));
        }

        // Does not stage files — commits only what is already staged.
        let output = run_bounded_git_command(
            &self.root,
            &["commit", "-m", message],
            MAX_GIT_COMMIT_STDOUT_BYTES,
            MAX_GIT_COMMIT_STDERR_BYTES,
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr.bytes);
            let stdout_str = String::from_utf8_lossy(&output.stdout.bytes);
            // Detect the common "nothing to commit" case and surface a clear error.
            if stderr.contains("nothing to commit")
                || stdout_str.contains("nothing to commit")
                || stderr.contains("nothing added to commit")
                || stdout_str.contains("nothing added to commit")
            {
                return Err(ToolError::InvalidInput(
                    "git_commit failed: nothing staged to commit".into(),
                ));
            }
            return Err(ToolError::InvalidInput(format!(
                "git commit failed: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout.bytes);
        let hash = parse_short_hash(&stdout);
        let files_committed = parse_files_committed(&stdout);
        let subject = message.lines().next().unwrap_or(message).to_string();

        Ok(ToolOutput::GitCommit(GitCommitOutput {
            hash,
            subject,
            files_committed,
        }))
    }
}

/// Extracts the short hash from a `git commit` first output line.
/// Format: `[<branch> <hash>] <subject>` or `[<branch> (root-commit) <hash>] <subject>`.
fn parse_short_hash(stdout: &str) -> String {
    stdout
        .lines()
        .next()
        .and_then(|line| {
            let bracket_content = line.split('[').nth(1)?.split(']').next()?;
            bracket_content
                .split_whitespace()
                .last()
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Extracts the number of changed files from a `git commit` output line.
/// Matches lines like ` 1 file changed, ...` or ` 3 files changed, ...`.
fn parse_files_committed(stdout: &str) -> usize {
    stdout
        .lines()
        .find(|line| line.contains("file") && line.contains("changed"))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0)
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
    ToolError::InvalidInput("git_commit failed: output capture failed".into())
}

fn git_command_error(error: io::Error) -> ToolError {
    if error.kind() == io::ErrorKind::NotFound {
        ToolError::InvalidInput("git_commit failed: git executable unavailable".into())
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

    fn approve_commit(path: &Path, message: &str) -> Result<ToolOutput, ToolError> {
        let tool = GitCommitTool::new(PathBuf::from(path));
        let result = tool.run(&ResolvedToolInput::GitCommit {
            message: message.to_string(),
        })?;
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected Approval");
        };
        tool.execute_approved(&pending.payload)
    }

    #[test]
    fn spec_requires_approval_high_risk() {
        let tool = GitCommitTool::new(PathBuf::from("."));
        let spec = tool.spec();
        assert_eq!(spec.name, "git_commit");
        assert_eq!(spec.execution_kind, ExecutionKind::RequiresApproval);
        assert_eq!(spec.default_risk, Some(RiskLevel::High));
    }

    #[test]
    fn rejects_empty_message_in_run() {
        let result = GitCommitTool::new(PathBuf::from(".")).run(&ResolvedToolInput::GitCommit {
            message: "".to_string(),
        });
        assert!(matches!(result.unwrap_err(), ToolError::InvalidInput(_)));
    }

    #[test]
    fn rejects_whitespace_only_message() {
        let result = GitCommitTool::new(PathBuf::from(".")).run(&ResolvedToolInput::GitCommit {
            message: "   \n  ".to_string(),
        });
        assert!(matches!(result.unwrap_err(), ToolError::InvalidInput(_)));
    }

    #[test]
    fn run_returns_approval_with_correct_risk() {
        let result = GitCommitTool::new(PathBuf::from("."))
            .run(&ResolvedToolInput::GitCommit {
                message: "Add feature X".to_string(),
            })
            .unwrap();
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected Approval");
        };
        assert_eq!(pending.tool_name, "git_commit");
        assert_eq!(pending.risk, RiskLevel::High);
        assert!(pending.summary.contains("Add feature X"));
        assert_eq!(pending.payload, "Add feature X");
    }

    #[test]
    fn commits_staged_changes() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        fs::write(tmp.path().join("file.txt"), "hello\n").unwrap();
        git(tmp.path(), &["add", "file.txt"]);

        let out = approve_commit(tmp.path(), "feat: add file.txt\n\nThis is the body.").unwrap();
        let ToolOutput::GitCommit(o) = out else {
            panic!("expected GitCommit");
        };
        assert!(!o.hash.is_empty(), "hash must be non-empty");
        assert_eq!(o.subject, "feat: add file.txt");
        assert!(o.files_committed >= 1);
    }

    #[test]
    fn parse_short_hash_standard_output() {
        let output = "[main abc1234] Add feature\n 1 file changed, 2 insertions(+)\n";
        assert_eq!(parse_short_hash(output), "abc1234");
    }

    #[test]
    fn parse_short_hash_root_commit_output() {
        let output =
            "[main (root-commit) abc1234] Initial commit\n 1 file changed, 1 insertion(+)\n";
        assert_eq!(parse_short_hash(output), "abc1234");
    }

    #[test]
    fn parse_files_committed_single_file() {
        let output = "[main abc1234] msg\n 1 file changed, 2 insertions(+)\n";
        assert_eq!(parse_files_committed(output), 1);
    }

    #[test]
    fn parse_files_committed_multiple_files() {
        let output = "[main abc1234] msg\n 3 files changed, 10 insertions(+), 2 deletions(-)\n";
        assert_eq!(parse_files_committed(output), 3);
    }
}
