use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::runtime::ResolvedToolInput;

use crate::tools::types::{
    ExecutionKind, GitDiffStagedOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use crate::tools::Tool;

const MAX_GIT_DIFF_STAGED_STDOUT_BYTES: usize = 128 * 1024;
const MAX_GIT_DIFF_STAGED_STDERR_BYTES: usize = 8 * 1024;

pub struct GitDiffStagedTool {
    root: PathBuf,
}

impl GitDiffStagedTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn run_diff_staged(&self) -> Result<ToolRunResult, ToolError> {
        let output = run_bounded_git_diff_staged(&self.root)?;

        if !output.status.success() {
            return Err(git_diff_staged_error(
                &output.stdout.bytes,
                &output.stderr.bytes,
            ));
        }

        Ok(ToolRunResult::Immediate(ToolOutput::GitDiffStaged(
            git_diff_staged_output(output.stdout),
        )))
    }
}

impl Tool for GitDiffStagedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_diff_staged",
            description: "Show read-only staged git diff (index vs HEAD) for the project.",
            input_hint: "",
            execution_kind: ExecutionKind::Immediate,
            default_risk: None,
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::GitDiffStaged = input else {
            return Err(ToolError::InvalidInput(
                "git_diff_staged received wrong input variant".into(),
            ));
        };

        self.run_diff_staged()
    }
}

struct BoundedGitOutput {
    status: ExitStatus,
    stdout: BoundedCapture,
    stderr: BoundedCapture,
}

struct BoundedCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

fn run_bounded_git_diff_staged(root: &std::path::Path) -> Result<BoundedGitOutput, ToolError> {
    let mut child = Command::new("git")
        .args([
            "diff",
            "--staged",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
        ])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(git_command_error)?;

    let stdout = child.stdout.take().ok_or_else(output_capture_error)?;
    let stderr = child.stderr.take().ok_or_else(output_capture_error)?;

    let stdout_reader =
        thread::spawn(move || read_bounded_stream(stdout, MAX_GIT_DIFF_STAGED_STDOUT_BYTES));
    let stderr_reader =
        thread::spawn(move || read_bounded_stream(stderr, MAX_GIT_DIFF_STAGED_STDERR_BYTES));

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

    Ok(BoundedCapture { bytes, truncated })
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
    ToolError::InvalidInput("git_diff_staged failed: output capture failed".into())
}

fn git_command_error(error: io::Error) -> ToolError {
    if error.kind() == io::ErrorKind::NotFound {
        ToolError::InvalidInput("git_diff_staged failed: git executable unavailable".into())
    } else {
        ToolError::Io(error)
    }
}

fn git_diff_staged_error(stdout: &[u8], stderr: &[u8]) -> ToolError {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_ascii_lowercase();
    if combined.contains("not a git repository") {
        ToolError::InvalidInput("git_diff_staged failed: not a Git repository".into())
    } else {
        ToolError::InvalidInput("git_diff_staged failed".into())
    }
}

fn git_diff_staged_output(capture: BoundedCapture) -> GitDiffStagedOutput {
    let patch = String::from_utf8_lossy(&capture.bytes).to_string();
    GitDiffStagedOutput {
        bytes_shown: capture.bytes.len(),
        patch,
        truncated: capture.truncated,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
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
                "initial",
            ],
        );
    }

    fn run_diff_staged(path: &Path) -> Result<ToolRunResult, ToolError> {
        GitDiffStagedTool::new(PathBuf::from(path)).run(&ResolvedToolInput::GitDiffStaged)
    }

    #[test]
    fn spec_is_immediate() {
        let tool = GitDiffStagedTool::new(PathBuf::from("."));
        let spec = tool.spec();
        assert_eq!(spec.name, "git_diff_staged");
        assert_eq!(spec.execution_kind, ExecutionKind::Immediate);
        assert!(spec.default_risk.is_none());
    }

    #[test]
    fn non_git_directory_returns_error() {
        let tmp = TempDir::new().unwrap();
        let err = run_diff_staged(tmp.path()).unwrap_err();
        // git diff --staged may report the error via stdout or stderr depending on version.
        assert!(
            matches!(&err, ToolError::InvalidInput(msg) if msg.starts_with("git_diff_staged failed")),
            "expected git_diff_staged failed error, got: {err:?}"
        );
    }

    #[test]
    fn staged_diff_empty_when_nothing_staged() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());

        let out = run_diff_staged(tmp.path()).unwrap();
        let ToolRunResult::Immediate(ToolOutput::GitDiffStaged(diff)) = out else {
            panic!("expected Immediate(GitDiffStaged)");
        };
        assert_eq!(diff.patch, "");
        assert_eq!(diff.bytes_shown, 0);
        assert!(!diff.truncated);
    }

    #[test]
    fn staged_diff_with_changes() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "file.txt", "old content\n");
        fs::write(tmp.path().join("file.txt"), "new content\n").unwrap();
        git(tmp.path(), &["add", "file.txt"]);

        let out = run_diff_staged(tmp.path()).unwrap();
        let ToolRunResult::Immediate(ToolOutput::GitDiffStaged(diff)) = out else {
            panic!("expected Immediate(GitDiffStaged)");
        };
        assert!(diff.patch.contains("diff --git a/file.txt b/file.txt"));
        assert!(diff.patch.contains("-old content"));
        assert!(diff.patch.contains("+new content"));
        assert_eq!(diff.bytes_shown, diff.patch.as_bytes().len());
        assert!(!diff.truncated);
    }

    #[test]
    fn unstaged_changes_do_not_appear_in_staged_diff() {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        commit_file(tmp.path(), "file.txt", "original\n");
        // Modify but do NOT stage
        fs::write(tmp.path().join("file.txt"), "modified\n").unwrap();

        let out = run_diff_staged(tmp.path()).unwrap();
        let ToolRunResult::Immediate(ToolOutput::GitDiffStaged(diff)) = out else {
            panic!("expected Immediate(GitDiffStaged)");
        };
        assert_eq!(
            diff.patch, "",
            "unstaged changes must not appear in staged diff"
        );
    }

    #[test]
    fn bounded_capture_marks_truncated_without_retaining_extra_bytes() {
        let input = vec![b'x'; MAX_GIT_DIFF_STAGED_STDERR_BYTES + 10];
        let capture =
            read_bounded_stream(Cursor::new(input), MAX_GIT_DIFF_STAGED_STDERR_BYTES).unwrap();

        assert_eq!(capture.bytes.len(), MAX_GIT_DIFF_STAGED_STDERR_BYTES);
        assert!(capture.truncated);
    }
}
