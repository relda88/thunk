use std::sync::mpsc;

use crate::app::AppContext;
use crate::runtime::{RuntimeEvent, RuntimeRequest};
use crate::storage::session::SessionMeta;

#[derive(Debug)]
pub(crate) enum WorkerCmd {
    Handle(RuntimeRequest),
    Reset,
    ListSessions,
    ClearSessions,
    RebuildFile(std::path::PathBuf),
    ProactiveScan,
}

pub(crate) enum WorkerReply {
    Event(RuntimeEvent),
    DeferredVerification(String),
    HandleOk,
    HandleErr(String),
    ResetOk,
    ResetErr(String),
    SessionsOk(Vec<SessionMeta>),
    SessionsErr(String),
    ClearOk,
    ClearErr(String),
}

pub(crate) fn run_worker(
    mut app: AppContext,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
    reply_tx: mpsc::Sender<WorkerReply>,
) {
    for cmd in cmd_rx {
        match cmd {
            WorkerCmd::Handle(req) => {
                let tx = reply_tx.clone();
                let result = app.handle(req, &mut |ev| {
                    let _ = tx.send(WorkerReply::Event(ev));
                });
                // Slice 50.7: this closure is duplicated (identically) in the RebuildFile arm
                // below, and mirrors the synchronous `Runtime::run_verify_command` in
                // engine.rs — deferred_verify defaults to true in production, so this is the
                // path that actually runs, not the engine.rs one. A future slice should
                // consolidate all three copies into one shared function; keep them identical
                // until then.
                if let Some((verify_cmd, root)) = app.verify_context() {
                    let bg_tx = reply_tx.clone();
                    std::thread::spawn(move || {
                        let parts: Vec<String> =
                            verify_cmd.split_whitespace().map(str::to_owned).collect();
                        let program = match parts.first() {
                            Some(p) => p,
                            None => return,
                        };
                        let is_cargo = program == "cargo";
                        let is_ruff = program == "ruff";
                        let mut args: Vec<String> = parts[1..].to_vec();
                        if is_cargo {
                            args.push("--message-format=json".to_owned());
                        } else if is_ruff {
                            args.push("--output-format=json".to_owned());
                        }
                        let msg = match std::process::Command::new(program)
                            .args(&args)
                            .current_dir(&root)
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .output()
                        {
                            Err(_) => format!("{verify_cmd}: unavailable"),
                            Ok(out) => {
                                let status_ok = out.status.success();
                                let mut combined =
                                    String::from_utf8_lossy(&out.stdout).into_owned();
                                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                                if combined.len() > 4000 {
                                    let boundary = combined
                                        .char_indices()
                                        .map(|(i, _)| i)
                                        .filter(|&i| i <= 4000)
                                        .last()
                                        .unwrap_or(0);
                                    combined.truncate(boundary);
                                }
                                if is_cargo {
                                    let diags =
                                        crate::runtime::diagnostics::parse_diagnostics(&combined);
                                    if diags.is_empty() {
                                        if status_ok {
                                            format!("{verify_cmd}: ok")
                                        } else {
                                            format!("{verify_cmd}: failed\n{combined}")
                                        }
                                    } else {
                                        let text =
                                            crate::runtime::diagnostics::format_diagnostics(&diags);
                                        format!("{verify_cmd}: failed\n{text}")
                                    }
                                } else if is_ruff {
                                    let diags = crate::runtime::diagnostics::parse_ruff_diagnostics(
                                        &combined,
                                    );
                                    if diags.is_empty() {
                                        if status_ok {
                                            format!("{verify_cmd}: ok")
                                        } else {
                                            format!("{verify_cmd}: failed\n{combined}")
                                        }
                                    } else {
                                        let text =
                                            crate::runtime::diagnostics::format_diagnostics(&diags);
                                        format!("{verify_cmd}: failed\n{text}")
                                    }
                                } else if status_ok && combined.trim().is_empty() {
                                    format!("{verify_cmd}: ok")
                                } else if combined.trim().is_empty() {
                                    // Non-zero exit with no output at all to show — this is
                                    // newly reachable now that exit status is checked; there
                                    // was previously no output-based signal for this case.
                                    format!("{verify_cmd}: failed (non-zero exit, no output)")
                                } else {
                                    combined
                                }
                            }
                        };
                        let _ = bg_tx.send(WorkerReply::DeferredVerification(msg));
                    });
                }
                match result {
                    Ok(()) => {
                        let _ = reply_tx.send(WorkerReply::HandleOk);
                    }
                    Err(e) => {
                        let _ = reply_tx.send(WorkerReply::HandleErr(e.to_string()));
                    }
                }
            }
            WorkerCmd::Reset => match app.reset() {
                Ok(()) => {
                    let _ = reply_tx.send(WorkerReply::ResetOk);
                }
                Err(e) => {
                    let _ = reply_tx.send(WorkerReply::ResetErr(e.to_string()));
                }
            },
            WorkerCmd::ListSessions => match app.list_sessions() {
                Ok(sessions) => {
                    let _ = reply_tx.send(WorkerReply::SessionsOk(sessions));
                }
                Err(e) => {
                    let _ = reply_tx.send(WorkerReply::SessionsErr(e.to_string()));
                }
            },
            WorkerCmd::ClearSessions => match app.clear_sessions() {
                Ok(()) => {
                    let _ = reply_tx.send(WorkerReply::ClearOk);
                }
                Err(e) => {
                    let _ = reply_tx.send(WorkerReply::ClearErr(e.to_string()));
                }
            },
            WorkerCmd::RebuildFile(path) => {
                app.rebuild_file(path);
                // Slice 50.7: duplicated (identically) from the Handle arm above — see the
                // comment there. Keep both copies (and the engine.rs synchronous mirror)
                // identical until a future slice consolidates them.
                if let Some((verify_cmd, root)) = app.verify_context() {
                    let bg_tx = reply_tx.clone();
                    std::thread::spawn(move || {
                        let parts: Vec<String> =
                            verify_cmd.split_whitespace().map(str::to_owned).collect();
                        let program = match parts.first() {
                            Some(p) => p,
                            None => return,
                        };
                        let is_cargo = program == "cargo";
                        let is_ruff = program == "ruff";
                        let mut args: Vec<String> = parts[1..].to_vec();
                        if is_cargo {
                            args.push("--message-format=json".to_owned());
                        } else if is_ruff {
                            args.push("--output-format=json".to_owned());
                        }
                        let msg = match std::process::Command::new(program)
                            .args(&args)
                            .current_dir(&root)
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .output()
                        {
                            Err(_) => format!("{verify_cmd}: unavailable"),
                            Ok(out) => {
                                let status_ok = out.status.success();
                                let mut combined =
                                    String::from_utf8_lossy(&out.stdout).into_owned();
                                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                                if combined.len() > 4000 {
                                    let boundary = combined
                                        .char_indices()
                                        .map(|(i, _)| i)
                                        .filter(|&i| i <= 4000)
                                        .last()
                                        .unwrap_or(0);
                                    combined.truncate(boundary);
                                }
                                if is_cargo {
                                    let diags =
                                        crate::runtime::diagnostics::parse_diagnostics(&combined);
                                    if diags.is_empty() {
                                        if status_ok {
                                            format!("{verify_cmd}: ok")
                                        } else {
                                            format!("{verify_cmd}: failed\n{combined}")
                                        }
                                    } else {
                                        let text =
                                            crate::runtime::diagnostics::format_diagnostics(&diags);
                                        format!("{verify_cmd}: failed\n{text}")
                                    }
                                } else if is_ruff {
                                    let diags = crate::runtime::diagnostics::parse_ruff_diagnostics(
                                        &combined,
                                    );
                                    if diags.is_empty() {
                                        if status_ok {
                                            format!("{verify_cmd}: ok")
                                        } else {
                                            format!("{verify_cmd}: failed\n{combined}")
                                        }
                                    } else {
                                        let text =
                                            crate::runtime::diagnostics::format_diagnostics(&diags);
                                        format!("{verify_cmd}: failed\n{text}")
                                    }
                                } else if status_ok && combined.trim().is_empty() {
                                    format!("{verify_cmd}: ok")
                                } else if combined.trim().is_empty() {
                                    // Non-zero exit with no output at all to show — this is
                                    // newly reachable now that exit status is checked; there
                                    // was previously no output-based signal for this case.
                                    format!("{verify_cmd}: failed (non-zero exit, no output)")
                                } else {
                                    combined
                                }
                            }
                        };
                        let _ = bg_tx.send(WorkerReply::DeferredVerification(msg));
                    });
                }
            }
            WorkerCmd::ProactiveScan => {
                let tx = reply_tx.clone();
                app.proactive_scan(&mut |ev| {
                    let _ = tx.send(WorkerReply::Event(ev));
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use crate::app::config::Config;
    use crate::app::paths::AppPaths;
    use crate::app::session::ActiveSession;
    use crate::app::AppContext;
    use crate::llm::providers::build_backend;
    use crate::runtime::{ProjectRoot, RuntimeRequest};
    use crate::tools::default_registry;
    use crate::tools::{PendingAction, RiskLevel};

    use super::{run_worker, WorkerCmd, WorkerReply};

    /// Builds a real AppContext backed by a real cargo project, with `deferred_verify` at
    /// its production default (true) — this is what actually exercises the
    /// `WorkerCmd::Handle` verify closure, since `Runtime::run_verify_command` (engine.rs)
    /// only runs synchronously in tests that explicitly opt out via `with_deferred_verify`.
    fn build_app(tmp: &TempDir, verify_command: &str) -> AppContext {
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "not valid toml [[[").unwrap();
        fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir_all(tmp.path().join("data")).unwrap();
        fs::create_dir_all(tmp.path().join("logs")).unwrap();

        let project_root = ProjectRoot::new(tmp.path().to_path_buf()).unwrap();
        let paths = AppPaths {
            root_dir: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
            project_label: None,
            thunk_dir: tmp.path().join(".thunk"),
            config_file: tmp.path().join("config.toml"),
            data_dir: tmp.path().join("data"),
            logs_dir: tmp.path().join("logs"),
            session_db: tmp.path().join("data").join("sessions.db"),
            home_mcp_config: None,
        };
        let mut config = Config::default();
        config.project.verify_command = Some(verify_command.to_string());
        let backend = build_backend(&config).unwrap();
        let registry = default_registry().with_project_root(project_root.as_path_buf());
        let (session, history, anchors) =
            ActiveSession::open_or_restore(&paths.session_db, &project_root).unwrap();
        AppContext::build(
            &config,
            project_root,
            backend,
            registry,
            session,
            history,
            anchors,
            None,
            Some(&paths.session_db),
            None,
            paths.thunk_dir.clone(),
        )
        .unwrap()
    }

    #[test]
    fn handle_cmd_verify_reports_failure_on_nonzero_exit_with_no_diagnostics() {
        // Slice 50.7's most important regression test: the actual live production path
        // (WorkerCmd::Handle's deferred-verify closure, reached because deferred_verify
        // defaults to true) must not report "ok" for a non-zero cargo exit with no
        // parseable diagnostics. Prior to this slice, this closure had zero test coverage
        // and shared the same exit-status-blind bug as engine.rs::run_verify_command.
        let tmp = TempDir::new().unwrap();
        let mut app = build_app(&tmp, "cargo check");

        let abs_path = tmp
            .path()
            .join("src")
            .join("main.rs")
            .to_string_lossy()
            .into_owned();
        let payload = format!("{abs_path}\x00fn main() {{}}\x00fn main() {{ let _x = 1; }}");
        app.runtime_mut().set_pending_for_test(PendingAction {
            tool_name: "edit_file".into(),
            summary: format!("edit {abs_path}"),
            risk: RiskLevel::Low,
            reversible: true,
            payload,
        });

        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let (reply_tx, reply_rx) = mpsc::channel::<WorkerReply>();
        std::thread::spawn(move || run_worker(app, cmd_rx, reply_tx));
        cmd_tx
            .send(WorkerCmd::Handle(RuntimeRequest::Approve))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut deferred_msg: Option<String> = None;
        while Instant::now() < deadline {
            match reply_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(WorkerReply::DeferredVerification(msg)) => {
                    deferred_msg = Some(msg);
                    break;
                }
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let msg = deferred_msg.expect(
            "must receive a DeferredVerification reply from the live WorkerCmd::Handle path",
        );
        assert_ne!(
            msg.trim(),
            "cargo check: ok",
            "a non-zero exit with no parseable diagnostics must never report 'ok' on the live \
             deferred-verify path: {msg}"
        );
        assert!(
            msg.contains("failed"),
            "must report failure on the live deferred-verify path: {msg}"
        );
    }
}
