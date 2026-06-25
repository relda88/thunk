use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::runtime::ResolvedToolInput;

use crate::tools::pending::{PendingAction, RiskLevel};
use crate::tools::types::{
    ExecutionKind, GitBranchCreateOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use crate::tools::Tool;

const MAX_GIT_BRANCH_CREATE_STDOUT_BYTES: usize = 16 * 1024;
const MAX_GIT_BRANCH_CREATE_STDERR_BYTES: usize = 4 * 1024;

// Payload separator: null byte cannot appear in branch names or ref names.
const SEP: char = '\x00';

pub struct GitBranchCreateTool {
    root: PathBuf,
}

impl GitBranchCreateTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

fn encode_payload(name: &str, start_point: Option<&str>) -> String {
    format!("{}{SEP}{}", name, start_point.unwrap_or(""))
}

fn decode_payload(payload: &str) -> Option<(String, Option<String>)> {
    let mut parts = payload.splitn(2, SEP);
    let name = parts.next()?.to_string();
    if name.is_empty() {
        return None;
    }
    let start_point = parts.next().filter(|s| !s.is_empty()).map(String::from);
    Some((name, start_point))
}

impl Tool for GitBranchCreateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_branch_create",
            description: "Create a new local git branch. Requires approval before executing.",
            input_hint: "",
            execution_kind: ExecutionKind::RequiresApproval,
            default_risk: Some(RiskLevel::Medium),
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::GitBranchCreate { name, start_point } = input else {
            return Err(ToolError::InvalidInput(
                "git_branch_create received wrong input variant".into(),
            ));
        };

        if name.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "git_branch_create: branch name must not be empty".into(),
            ));
        }

        let summary = match start_point.as_deref() {
            Some(sp) => format!("create branch {name} from {sp}"),
            None => format!("create branch {name}"),
        };

        Ok(ToolRunResult::Approval(PendingAction {
            tool_name: "git_branch_create".to_string(),
            summary,
            risk: RiskLevel::Medium,
            reversible: true,
            payload: encode_payload(name, start_point.as_deref()),
        }))
    }

    fn execute_approved(&self, payload: &str) -> Result<ToolOutput, ToolError> {
        let (name, start_point) = decode_payload(payload)
            .ok_or_else(|| ToolError::InvalidInput("malformed git_branch_create payload".into()))?;

        // Revalidation: verify branch does not already exist.
        let list_output = run_bounded_git_command(
            &self.root,
            &["branch", "--list", &name],
            MAX_GIT_BRANCH_CREATE_STDOUT_BYTES,
            MAX_GIT_BRANCH_CREATE_STDERR_BYTES,
        )?;
        let list_stdout = String::from_utf8_lossy(&list_output.stdout.bytes);
        if !list_stdout.trim().is_empty() {
            return Err(ToolError::InvalidInput(format!(
                "git_branch_create failed: branch '{name}' already exists"
            )));
        }

        let mut args = vec!["branch", name.as_str()];
        if let Some(ref sp) = start_point {
            args.push(sp.as_str());
        }

        let output = run_bounded_git_command(
            &self.root,
            &args,
            MAX_GIT_BRANCH_CREATE_STDOUT_BYTES,
            MAX_GIT_BRANCH_CREATE_STDERR_BYTES,
        )?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr.bytes);
            return Err(ToolError::InvalidInput(format!(
                "git branch failed: {stderr}"
            )));
        }

        Ok(ToolOutput::GitBranchCreate(GitBranchCreateOutput { name }))
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
    ToolError::InvalidInput("git_branch_create failed: output capture failed".into())
}

fn git_command_error(error: io::Error) -> ToolError {
    if error.kind() == io::ErrorKind::NotFound {
        ToolError::InvalidInput("git_branch_create failed: git executable unavailable".into())
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

    fn branch_exists(path: &Path, name: &str) -> bool {
        let output = Command::new("git")
            .args(["branch", "--list", name])
            .current_dir(path)
            .output()
            .unwrap();
        !String::from_utf8_lossy(&output.stdout).trim().is_empty()
    }

    fn run_create(
        path: &Path,
        name: &str,
        start_point: Option<&str>,
    ) -> Result<ToolRunResult, ToolError> {
        GitBranchCreateTool::new(PathBuf::from(path)).run(&ResolvedToolInput::GitBranchCreate {
            name: name.to_string(),
            start_point: start_point.map(String::from),
        })
    }

    fn approve_create(
        path: &Path,
        name: &str,
        start_point: Option<&str>,
    ) -> Result<ToolOutput, ToolError> {
        let tool = GitBranchCreateTool::new(PathBuf::from(path));
        let result = tool.run(&ResolvedToolInput::GitBranchCreate {
            name: name.to_string(),
            start_point: start_point.map(String::from),
        })?;
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected Approval");
        };
        tool.execute_approved(&pending.payload)
    }

    #[test]
    fn spec_requires_approval() {
        let tool = GitBranchCreateTool::new(PathBuf::from("."));
        let spec = tool.spec();
        assert_eq!(spec.name, "git_branch_create");
        assert_eq!(spec.execution_kind, ExecutionKind::RequiresApproval);
        assert_eq!(spec.default_risk, Some(RiskLevel::Medium));
    }

    #[test]
    fn run_returns_approval_with_correct_fields() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "a\n");

        let result = run_create(tmp.path(), "feat/test", None).unwrap();
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected Approval");
        };
        assert_eq!(pending.tool_name, "git_branch_create");
        assert_eq!(pending.risk, RiskLevel::Medium);
        assert!(pending.summary.contains("feat/test"));
    }

    #[test]
    fn creates_branch() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "a\n");

        let out = approve_create(tmp.path(), "feat/test", None).unwrap();
        let ToolOutput::GitBranchCreate(o) = out else {
            panic!("expected GitBranchCreate");
        };
        assert_eq!(o.name, "feat/test");
        assert!(branch_exists(tmp.path(), "feat/test"));
    }

    #[test]
    fn rejects_existing_branch() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "a\n");
        // Create the branch first
        git(tmp.path(), &["branch", "existing"]);

        let err = approve_create(tmp.path(), "existing", None).unwrap_err();
        assert!(matches!(
            err,
            ToolError::InvalidInput(ref msg) if msg.contains("already exists")
        ));
    }

    #[test]
    fn rejects_empty_branch_name() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());

        let err = run_create(tmp.path(), "", None).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn payload_roundtrip_with_start_point() {
        let payload = encode_payload("feat/x", Some("main"));
        let (name, sp) = decode_payload(&payload).unwrap();
        assert_eq!(name, "feat/x");
        assert_eq!(sp.as_deref(), Some("main"));
    }

    #[test]
    fn payload_roundtrip_without_start_point() {
        let payload = encode_payload("feat/x", None);
        let (name, sp) = decode_payload(&payload).unwrap();
        assert_eq!(name, "feat/x");
        assert!(sp.is_none());
    }
}
