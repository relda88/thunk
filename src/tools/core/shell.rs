use std::io::{Error, ErrorKind, Read};
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

use crate::tools::pending::{PendingAction, RiskLevel};
use crate::tools::types::{
    ExecutionKind, ShellOutput, ToolError, ToolOutput, ToolRunResult, ToolSpec,
};
use crate::tools::Tool;

const OUTPUT_CAP_BYTES: usize = 8192;
#[cfg(not(test))]
const COMMAND_TIMEOUT_SECS: u64 = 60;
#[cfg(test)]
const COMMAND_TIMEOUT_SECS: u64 = 1;

pub struct ShellTool {
    project_root: PathBuf,
}

impl ShellTool {
    pub fn new(project_root: PathBuf) -> Self {
        let project_root = project_root.canonicalize().unwrap_or(project_root);
        Self { project_root }
    }
}

impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell",
            description: "Run a shell command. Commands are tiered by safety: read-only commands (ls, find, cat, grep, etc.) should use [shell_read: ...] instead. Filesystem mutations (mkdir, rmdir, cp, mv) require approval and are reversible. Arbitrary execution (bash, rm, unknown programs) requires approval, is irreversible, and requires /exec on. For pipes, globs, or redirects use bash -c via /exec on.",
            input_hint: "[shell: mkdir build]",
            execution_kind: ExecutionKind::RequiresApproval,
            default_risk: Some(RiskLevel::High),
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::Shell { command } = input else {
            return Err(ToolError::InvalidInput(
                "shell received wrong input variant".into(),
            ));
        };

        if command.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "shell command cannot be empty".into(),
            ));
        }

        let tier = classify_shell_tier(command);

        if matches!(tier, ShellTier::ReadOnly) {
            return Err(ToolError::InvalidInput(
                "read-only command — use [shell_read: ...] instead".to_string(),
            ));
        }

        let reversible = matches!(tier, ShellTier::FsMutation);
        let summary = format!("run: {}", command);

        Ok(ToolRunResult::Approval(PendingAction {
            tool_name: "shell".to_string(),
            summary,
            risk: RiskLevel::High,
            reversible,
            payload: command.clone(),
        }))
    }

    fn execute_approved(&self, payload: &str) -> Result<ToolOutput, ToolError> {
        let mut parts = payload.split_whitespace();
        let Some(program) = parts.next() else {
            return Err(ToolError::InvalidInput(
                "shell command cannot be empty".into(),
            ));
        };
        let args: Vec<String> = parts.map(str::to_string).collect();

        let mut child = Command::new(program)
            .args(&args)
            .current_dir(&self.project_root)
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

            let mut child = child_for_timeout.lock().expect("shell child lock poisoned");
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
                let mut child = child.lock().expect("shell child lock poisoned");
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
            .map_err(|_| ToolError::Io(Error::other("shell timeout thread panicked")))?;

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

        Ok(ToolOutput::Shell(ShellOutput {
            command: payload.to_string(),
            stdout_stderr,
            exit_code,
            truncated,
            total_bytes,
            timed_out,
        }))
    }
}

fn read_all<R: Read>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(handle: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>, ToolError> {
    handle
        .join()
        .map_err(|_| ToolError::Io(Error::other("shell reader thread panicked")))?
        .map_err(ToolError::Io)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::tools::pending::RiskLevel;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn tool_in(dir: &TempDir) -> ShellTool {
        ShellTool::new(dir.path().to_path_buf())
    }

    fn run_shell(tool: &ShellTool, command: &str) -> Result<ToolRunResult, ToolError> {
        tool.run(&ResolvedToolInput::Shell {
            command: command.to_string(),
        })
    }

    #[cfg(unix)]
    fn write_script(dir: &TempDir, stem: &str, body: &str) -> String {
        let file_name = format!("{stem}.sh");
        let path = dir.path().join(&file_name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();

        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();

        format!("./{file_name}")
    }

    #[cfg(windows)]
    fn write_script(dir: &TempDir, stem: &str, body: &str) -> String {
        let file_name = format!("{stem}.cmd");
        let path = dir.path().join(&file_name);
        fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
        file_name
    }

    #[test]
    fn run_returns_approval() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);

        let result = run_shell(&tool, "mkdir foo").unwrap();
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected approval");
        };

        assert_eq!(pending.tool_name, "shell");
        assert_eq!(pending.summary, "run: mkdir foo");
        assert_eq!(pending.risk, RiskLevel::High);
        assert!(pending.reversible);
        assert_eq!(pending.payload, "mkdir foo");
    }

    #[test]
    fn exec_tier_returns_irreversible_approval() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);

        let result = run_shell(&tool, "bash -c 'echo hi'").unwrap();
        let ToolRunResult::Approval(pending) = result else {
            panic!("expected approval");
        };

        assert!(!pending.reversible);
        assert_eq!(pending.risk, RiskLevel::High);
    }

    #[test]
    fn readonly_tier_is_rejected_with_steer_error() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);

        let result = run_shell(&tool, "ls .");
        assert!(
            matches!(result, Err(ToolError::InvalidInput(_))),
            "expected InvalidInput for read-only command"
        );
    }

    #[test]
    fn execute_approved_successful_command_returns_exit_zero_and_output() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);
        let command = write_script(
            &dir,
            "success",
            &success_script_body("hello stdout", "hello stderr"),
        );

        let ToolOutput::Shell(output) = tool.execute_approved(&command).unwrap() else {
            panic!("expected shell output");
        };

        assert_eq!(output.command, command);
        assert_eq!(output.exit_code, 0);
        assert!(!output.truncated);
        assert!(!output.timed_out);
        assert!(output.stdout_stderr.contains("hello stdout"));
        assert!(output.stdout_stderr.contains("hello stderr"));
    }

    #[test]
    fn execute_approved_failed_command_returns_exit_one() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);
        let command = write_script(&dir, "fail", &failing_script_body("hello failure"));

        let ToolOutput::Shell(output) = tool.execute_approved(&command).unwrap() else {
            panic!("expected shell output");
        };

        assert_eq!(output.exit_code, 1);
        assert!(!output.truncated);
        assert!(!output.timed_out);
        assert!(output.stdout_stderr.contains("hello failure"));
    }

    #[test]
    fn execute_approved_truncates_output_over_8kb() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);
        let command = write_script(&dir, "large", &large_output_script_body());

        let ToolOutput::Shell(output) = tool.execute_approved(&command).unwrap() else {
            panic!("expected shell output");
        };

        assert_eq!(output.exit_code, 0);
        assert!(output.truncated);
        assert_eq!(output.stdout_stderr.len(), OUTPUT_CAP_BYTES);
        assert!(output.total_bytes > OUTPUT_CAP_BYTES);
        assert!(!output.timed_out);
    }

    #[test]
    fn execute_approved_times_out() {
        let dir = TempDir::new().unwrap();
        let tool = tool_in(&dir);
        let command = write_script(&dir, "sleep", &timeout_script_body());

        let ToolOutput::Shell(output) = tool.execute_approved(&command).unwrap() else {
            panic!("expected shell output");
        };

        assert_eq!(output.exit_code, -1);
        assert!(output.timed_out);
    }

    #[cfg(unix)]
    fn success_script_body(stdout: &str, stderr: &str) -> String {
        format!("printf '{stdout}\\n'\nprintf '{stderr}\\n' >&2")
    }

    #[cfg(windows)]
    fn success_script_body(stdout: &str, stderr: &str) -> String {
        format!("echo {stdout}\r\necho {stderr} 1>&2")
    }

    #[cfg(unix)]
    fn failing_script_body(message: &str) -> String {
        format!("printf '{message}\\n' >&2\nexit 1")
    }

    #[cfg(windows)]
    fn failing_script_body(message: &str) -> String {
        format!("echo {message} 1>&2\r\nexit /b 1")
    }

    #[cfg(unix)]
    fn large_output_script_body() -> String {
        "i=0\nwhile [ \"$i\" -lt 9000 ]\ndo\n  printf 'a'\n  i=$((i + 1))\ndone".to_string()
    }

    #[cfg(windows)]
    fn large_output_script_body() -> String {
        "for /L %%i in (1,1,9000) do <nul set /p =a".to_string()
    }

    #[cfg(unix)]
    fn timeout_script_body() -> String {
        "sleep 2".to_string()
    }

    #[cfg(windows)]
    fn timeout_script_body() -> String {
        "timeout /t 2 /nobreak >NUL".to_string()
    }
}
