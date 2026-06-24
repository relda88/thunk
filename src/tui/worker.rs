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
}

pub(super) enum WorkerReply {
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

pub(super) fn run_worker(
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
                                        format!("{verify_cmd}: ok")
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
                                        format!("{verify_cmd}: ok")
                                    } else {
                                        let text =
                                            crate::runtime::diagnostics::format_diagnostics(&diags);
                                        format!("{verify_cmd}: failed\n{text}")
                                    }
                                } else if combined.trim().is_empty() {
                                    format!("{verify_cmd}: ok")
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
            }
        }
    }
}
