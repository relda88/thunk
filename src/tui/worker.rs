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
}

pub(super) enum WorkerReply {
    Event(RuntimeEvent),
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
        }
    }
}
