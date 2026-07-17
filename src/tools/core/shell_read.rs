use std::io::{Error, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use crate::runtime::ResolvedToolInput;
use crate::runtime::{classify_shell_tier, ShellTier};
use crate::tools::types::{
    ExecutionKind, ShellOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use crate::tools::Tool;

const OUTPUT_CAP_BYTES: usize = 8192;
#[cfg(not(test))]
const COMMAND_TIMEOUT_SECS: u64 = 60;
#[cfg(test)]
const COMMAND_TIMEOUT_SECS: u64 = 1;

pub struct ShellReadTool {
    project_root: PathBuf,
}

impl ShellReadTool {
    pub fn new(project_root: PathBuf) -> Self {
        let project_root = project_root.canonicalize().unwrap_or(project_root);
        Self { project_root }
    }
}

impl Tool for ShellReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell_read",
            description: "Run a read-only shell command (ls, find, cat, grep, wc, head, tail, sed without -i). No pipes, globs, or redirects — use bash -c via /exec on for those. Commands that are not read-only are rejected. Arguments are confined to the project root: a command whose argument points at an existing path outside the root is rejected — use mcp::filesystem tools for those instead.",
            input_hint: "[shell_read: ls src/]",
            execution_kind: ExecutionKind::Immediate,
            default_risk: None,
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::ShellRead { command } = input else {
            return Err(ToolError::InvalidInput(
                "shell_read received wrong input variant".into(),
            ));
        };

        if command.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "shell_read command cannot be empty".into(),
            ));
        }

        if classify_shell_tier(command) != ShellTier::ReadOnly {
            return Err(ToolError::InvalidInput(
                "not a read-only command — use [shell: ...] for mutations or /exec on for arbitrary execution".into(),
            ));
        }

        run_shell_read(&self.project_root, command)
    }
}

fn run_shell_read(project_root: &PathBuf, command: &str) -> Result<ToolRunResult, ToolError> {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else {
        return Err(ToolError::InvalidInput(
            "shell_read command cannot be empty".into(),
        ));
    };
    let args: Vec<String> = parts.map(str::to_string).collect();

    let mut child = Command::new(program)
        .args(&args)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Io(Error::other("failed to capture stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::Io(Error::other("failed to capture stderr")))?;

    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));

    let child = Arc::new(Mutex::new(child));
    let timed_out = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();

    let child_for_timeout = Arc::clone(&child);
    let timed_out_for_timeout = Arc::clone(&timed_out);
    let timeout_thread = thread::spawn(move || {
        if done_rx
            .recv_timeout(Duration::from_secs(COMMAND_TIMEOUT_SECS))
            .is_ok()
        {
            return;
        }

        let mut child = child_for_timeout
            .lock()
            .expect("shell_read child lock poisoned");
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                timed_out_for_timeout.store(true, Ordering::SeqCst);
                let _ = child.kill();
            }
            Err(_) => {}
        }
    });

    let status = loop {
        let maybe_status = {
            let mut child = child.lock().expect("shell_read child lock poisoned");
            child.try_wait()?
        };
        if let Some(status) = maybe_status {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };

    let _ = done_tx.send(());
    timeout_thread
        .join()
        .map_err(|_| ToolError::Io(Error::other("shell_read timeout thread panicked")))?;

    let mut combined = join_reader(stdout_reader)?;
    combined.extend(join_reader(stderr_reader)?);

    let total_bytes = combined.len();
    let truncated = total_bytes > OUTPUT_CAP_BYTES;
    if truncated {
        combined.truncate(OUTPUT_CAP_BYTES);
    }

    let timed_out = timed_out.load(Ordering::SeqCst);
    let exit_code = if timed_out {
        -1
    } else {
        status.code().unwrap_or(-1)
    };
    let stdout_stderr = String::from_utf8_lossy(&combined).into_owned();

    Ok(ToolRunResult::Immediate(ToolOutput::ShellRead(
        ShellOutput {
            command: command.to_string(),
            stdout_stderr,
            exit_code,
            truncated,
            total_bytes,
            timed_out,
        },
    )))
}

fn read_all<R: Read>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(handle: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>, ToolError> {
    handle
        .join()
        .map_err(|_| ToolError::Io(Error::other("shell_read reader thread panicked")))?
        .map_err(ToolError::Io)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn make_tool() -> (ShellReadTool, TempDir) {
        let dir = TempDir::new().unwrap();
        let tool = ShellReadTool::new(dir.path().to_path_buf());
        (tool, dir)
    }

    #[test]
    fn run_returns_immediate_for_read_only_command() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::ShellRead {
            command: "echo hello".to_string(),
        };
        let result = tool.run(&input).unwrap();
        assert!(matches!(result, ToolRunResult::Immediate(_)));
    }

    #[test]
    fn run_rejects_non_read_only_command() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::ShellRead {
            command: "rm -rf foo".to_string(),
        };
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn run_rejects_mutation_command() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::ShellRead {
            command: "mkdir foo".to_string(),
        };
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn run_rejects_empty_command() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::ShellRead {
            command: "".to_string(),
        };
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn run_rejects_wrong_input_variant() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::GitStatus;
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn spec_has_correct_name_and_kind() {
        let (tool, _dir) = make_tool();
        let spec = tool.spec();
        assert_eq!(spec.name, "shell_read");
        assert_eq!(spec.execution_kind, ExecutionKind::Immediate);
        assert!(spec.default_risk.is_none());
    }

    #[test]
    fn run_rejects_sed_with_inplace_flag() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::ShellRead {
            command: "sed -i s/foo/bar/ file.txt".to_string(),
        };
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn run_rejects_command_with_redirect() {
        let (tool, _dir) = make_tool();
        let input = ResolvedToolInput::ShellRead {
            command: "echo foo > out.txt".to_string(),
        };
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
