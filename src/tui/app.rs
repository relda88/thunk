use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};

use crate::app::paths::AppPaths;
use crate::app::AppContext;
use crate::core::config::Config;
use crate::core::error::Result;
use crate::runtime::RuntimeEvent;

use super::cursor::{sync_terminal_affordances, CursorShape};
use super::keybindings::handle_key_event;
use super::renderer::Renderer;
use super::state::{AppState, DirtySections};
use super::worker::{run_worker, WorkerCmd, WorkerReply};
use super::{events, format};

const ACTIVE_MS: u64 = 33;
const SLOW_MS: u64 = 66;
const IDLE_MS: u64 = 180;

struct RenderScheduler {
    last_draw: Instant,
    heavy_streak: u32,
}

impl RenderScheduler {
    fn new() -> Self {
        Self {
            last_draw: Instant::now() - Duration::from_millis(IDLE_MS),
            heavy_streak: 0,
        }
    }

    fn poll_timeout(&self, state: &AppState) -> Duration {
        if state.has_dirty_sections() {
            return Duration::ZERO;
        }
        let interval = self.interval(state);
        interval.saturating_sub(self.last_draw.elapsed())
    }

    fn should_draw(&self, state: &AppState) -> bool {
        state.has_dirty_sections() || self.last_draw.elapsed() >= self.interval(state)
    }

    fn record_draw(&mut self, elapsed_ms: u64) {
        self.last_draw = Instant::now();
        if elapsed_ms > 24 {
            self.heavy_streak = self.heavy_streak.saturating_add(1);
        } else {
            self.heavy_streak = 0;
        }
    }

    fn interval(&self, state: &AppState) -> Duration {
        if state.is_busy {
            if self.heavy_streak > 3 {
                Duration::from_millis(SLOW_MS)
            } else {
                Duration::from_millis(ACTIVE_MS)
            }
        } else {
            Duration::from_millis(IDLE_MS)
        }
    }
}

pub(crate) fn run_app(
    stdout: &mut io::Stdout,
    config: &Config,
    paths: &AppPaths,
    app: AppContext,
) -> Result<()> {
    let mut state = AppState::new(config, paths);
    let (w, h) = crossterm::terminal::size()?;
    let mut renderer = Renderer::new(w, h);
    let mut scheduler = RenderScheduler::new();
    let mut last_cursor_shape: Option<CursorShape> = None;

    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (reply_tx, reply_rx) = mpsc::channel::<WorkerReply>();
    thread::spawn(move || run_worker(app, cmd_rx, reply_tx));

    {
        let watcher_cmd_tx = cmd_tx.clone();
        let watch_root = paths.project_root.clone();
        thread::spawn(move || {
            let (watcher_tx, watcher_rx) = mpsc::channel::<notify::Result<notify::Event>>();
            let mut watcher =
                match notify::RecommendedWatcher::new(watcher_tx, notify::Config::default()) {
                    Ok(w) => w,
                    Err(_) => return,
                };
            use notify::Watcher as _;
            if watcher
                .watch(&watch_root, notify::RecursiveMode::Recursive)
                .is_err()
            {
                return;
            }
            for res in watcher_rx {
                match res {
                    Ok(event) => {
                        for path in event.paths {
                            if should_rebuild(&path, &watch_root) {
                                if watcher_cmd_tx.send(WorkerCmd::RebuildFile(path)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(_) => return,
                }
            }
        });
    }

    {
        let proactive_cmd_tx = cmd_tx.clone();
        let proactive_enabled = config.proactive.enabled;
        let interval_secs: u64 = config.proactive.interval_secs;
        if proactive_enabled {
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(interval_secs));
                let _ = proactive_cmd_tx.send(WorkerCmd::ProactiveScan);
            });
        }
    }

    loop {
        while let Ok(reply) = reply_rx.try_recv() {
            handle_worker_reply(&mut state, reply);
        }

        if scheduler.should_draw(&state) {
            let t = Instant::now();
            sync_terminal_affordances(&state, &mut last_cursor_shape, stdout)?;
            let dirty = state.dirty_sections;
            renderer.render(&mut state, stdout, dirty)?;
            state.clear_dirty_sections();
            scheduler.record_draw(t.elapsed().as_millis() as u64);
        }

        if state.should_quit {
            return Ok(());
        }

        if event::poll(scheduler.poll_timeout(&state))? {
            match event::read()? {
                Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                    handle_key_event(&mut state, &cmd_tx, config, key)?
                }
                Event::Paste(text) => state.insert_str(&AppState::normalized_paste(&text)),
                Event::Resize(w, h) => {
                    renderer.resize(w, h);
                    state.mark_dirty(DirtySections::ALL);
                }
                _ => {}
            }
        }
    }
}

fn handle_worker_reply(state: &mut AppState, reply: WorkerReply) {
    match reply {
        WorkerReply::Event(ev) => events::apply_runtime_event(state, ev),
        WorkerReply::DeferredVerification(msg) => state.add_system_message(msg),
        WorkerReply::HandleOk => state.is_busy = false,
        WorkerReply::HandleErr(msg) => {
            events::apply_runtime_event(state, RuntimeEvent::Failed { message: msg });
            state.is_busy = false;
        }
        WorkerReply::ResetOk => state.is_busy = false,
        WorkerReply::ResetErr(e) => {
            state.add_system_message(format!("session reset failed: {e}"));
            state.is_busy = false;
        }
        WorkerReply::SessionsOk(sessions) => {
            state.add_system_message(format::format_sessions_list(&sessions));
            state.is_busy = false;
        }
        WorkerReply::SessionsErr(e) => {
            state.set_status("error");
            state.add_system_message(format!("session list failed: {e}"));
            state.is_busy = false;
        }
        WorkerReply::ClearOk => {
            state.set_status("ready");
            state.add_system_message("current project sessions cleared; started fresh session");
            state.is_busy = false;
        }
        WorkerReply::ClearErr(e) => {
            state.set_status("error");
            state.add_system_message(format!("session clear failed: {e}"));
            state.is_busy = false;
        }
    }
}

fn should_rebuild(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.extension().map_or(false, |e| e == "rs")
        && path.starts_with(root)
        && !path
            .components()
            .any(|c| c.as_os_str() == "target" || c.as_os_str() == ".git")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::app::config::Config;
    use crate::app::paths::AppPaths;
    use crate::app::session::ActiveSession;
    use crate::app::AppContext;
    use crate::llm::providers::build_backend;
    use crate::runtime::{ProjectRoot, RuntimeRequest};
    use crate::storage::session::{SessionStore, StoredMessage};
    use crate::tools::default_registry;

    use super::{handle_key_event, handle_worker_reply, WorkerCmd};
    use crate::tui::state::{AppState, ApprovalRisk, PendingApprovalState};
    use crate::tui::worker::WorkerReply;

    #[test]
    fn session_clear_removes_old_project_sessions_and_leaves_fresh_active_session() {
        let mut harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);
        state.add_user_message("stale user message");
        state.add_assistant_message("stale assistant message");

        harness
            .app
            .handle(
                RuntimeRequest::Submit {
                    text: "before clear".into(),
                },
                &mut |_| {},
            )
            .unwrap();
        harness.app.reset().unwrap();
        harness
            .app
            .handle(
                RuntimeRequest::Submit {
                    text: "second session".into(),
                },
                &mut |_| {},
            )
            .unwrap();

        let other_root = TempDir::new().unwrap();
        let other_root = other_root.path().canonicalize().unwrap();
        let store = SessionStore::open(&harness.paths.session_db).unwrap();
        let foreign = store.create(&other_root).unwrap();
        store
            .save(
                &foreign.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "foreign session".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        // Exercise the ClearProjectSessions path directly (handle_command now routes
        // through the worker channel; tests call the underlying operations inline).
        state.clear_messages();
        match harness.app.clear_sessions() {
            Ok(()) => {
                state.set_status("ready");
                state.add_system_message("current project sessions cleared; started fresh session");
            }
            Err(e) => {
                state.set_status("error");
                state.add_system_message(format!("session clear failed: {e}"));
            }
        }

        assert_eq!(state.messages.len(), 2);
        assert!(state.messages[0].content.contains("ready. Root:"));
        assert_eq!(
            state.messages[1].content,
            "current project sessions cleared; started fresh session"
        );
        assert_eq!(state.status, "ready");
        assert!(state
            .messages
            .iter()
            .all(|m| !m.content.contains("stale user message")));
        assert!(state
            .messages
            .iter()
            .all(|m| !m.content.contains("stale assistant message")));

        let sessions_after_clear = harness.app.list_sessions().unwrap();
        assert_eq!(sessions_after_clear.len(), 1);
        assert_eq!(sessions_after_clear[0].message_count, 0);

        harness
            .app
            .handle(
                RuntimeRequest::Submit {
                    text: "after clear".into(),
                },
                &mut |_| {},
            )
            .unwrap();

        let sessions_after_submit = harness.app.list_sessions().unwrap();
        assert_eq!(sessions_after_submit.len(), 1);
        assert_eq!(sessions_after_submit[0].message_count, 2);
        assert_eq!(
            store
                .list_for_project(other_root.to_string_lossy().as_ref())
                .unwrap()
                .len(),
            1
        );
    }

    struct TestHarness {
        _root_dir: TempDir,
        config: Config,
        paths: AppPaths,
        app: AppContext,
    }

    impl TestHarness {
        fn new() -> Self {
            let root_dir = TempDir::new().unwrap();
            fs::create_dir_all(root_dir.path().join("data")).unwrap();
            fs::create_dir_all(root_dir.path().join("logs")).unwrap();

            let project_root = ProjectRoot::new(root_dir.path().to_path_buf()).unwrap();
            let paths = AppPaths {
                root_dir: root_dir.path().to_path_buf(),
                project_root: root_dir.path().to_path_buf(),
                project_label: None,
                thunk_dir: root_dir.path().join(".thunk"),
                config_file: root_dir.path().join("config.toml"),
                data_dir: root_dir.path().join("data"),
                logs_dir: root_dir.path().join("logs"),
                session_db: root_dir.path().join("data").join("sessions.db"),
                home_mcp_config: None,
            };
            let config = Config::default();
            let backend = build_backend(&config).unwrap();
            let registry = default_registry().with_project_root(project_root.as_path_buf());
            let (session, history, anchors) =
                ActiveSession::open_or_restore(&paths.session_db, &project_root).unwrap();
            let app = AppContext::build(
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
            .unwrap();

            Self {
                _root_dir: root_dir,
                config,
                paths,
                app,
            }
        }
    }

    #[test]
    fn deferred_verification_reply_appends_system_message() {
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);
        let initial_count = state.messages.len();
        handle_worker_reply(
            &mut state,
            WorkerReply::DeferredVerification("cargo check: ok".to_string()),
        );
        assert_eq!(state.messages.len(), initial_count + 1);
        assert_eq!(state.messages.last().unwrap().content, "cargo check: ok");
        // is_busy must remain unchanged — DeferredVerification never touches it
        assert!(!state.is_busy);
    }

    fn make_key(
        code: crossterm::event::KeyCode,
        mods: crossterm::event::KeyModifiers,
    ) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: mods,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn ctrl_n_with_pending_approval_dispatches_reject() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "shell".into(),
            summary: "run tests".into(),
            risk: ApprovalRisk::High,
            irreversible: false,
            evidence: vec![],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
        });

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCmd>();
        let key = make_key(KeyCode::Char('n'), KeyModifiers::CONTROL);
        handle_key_event(&mut state, &cmd_tx, &harness.config, key).unwrap();

        assert!(state.is_busy, "dispatch must set is_busy");
        match cmd_rx.try_recv().expect("command must be sent") {
            WorkerCmd::Handle(RuntimeRequest::Reject) => {}
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn clear_messages_resets_pending_approval() {
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "shell".into(),
            summary: "run tests".into(),
            risk: ApprovalRisk::High,
            irreversible: false,
            evidence: vec![],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
        });
        assert!(state.pending_approval.is_some());
        state.clear_messages();
        assert!(
            state.pending_approval.is_none(),
            "clear_messages must reset pending_approval"
        );
    }

    #[test]
    fn ctrl_n_without_pending_approval_calls_recall_next_input() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);
        assert!(state.pending_approval.is_none());

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCmd>();
        let key = make_key(KeyCode::Char('n'), KeyModifiers::CONTROL);
        handle_key_event(&mut state, &cmd_tx, &harness.config, key).unwrap();

        assert!(!state.is_busy, "must not dispatch when no pending approval");
        assert!(cmd_rx.try_recv().is_err(), "no command must be sent");
    }

    #[test]
    fn ctrl_y_with_pending_approval_dispatches_approve() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let harness = TestHarness::new();
        let mut state = AppState::new(&harness.config, &harness.paths);
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "edit_file".into(),
            summary: "patch".into(),
            risk: ApprovalRisk::Medium,
            irreversible: false,
            evidence: vec![],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
        });

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCmd>();
        let key = make_key(KeyCode::Char('y'), KeyModifiers::CONTROL);
        handle_key_event(&mut state, &cmd_tx, &harness.config, key).unwrap();

        assert!(state.is_busy, "dispatch must set is_busy");
        match cmd_rx.try_recv().expect("command must be sent") {
            WorkerCmd::Handle(RuntimeRequest::Approve) => {}
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn should_rebuild_filter() {
        use std::path::PathBuf;
        let root = PathBuf::from("/proj");

        // source file inside project — passes
        assert!(super::should_rebuild(&root.join("src/lib.rs"), &root));

        // nested source file — passes
        assert!(super::should_rebuild(&root.join("src/tui/app.rs"), &root));

        // inside target/ — excluded
        assert!(!super::should_rebuild(
            &root.join("target/debug/build/foo.rs"),
            &root
        ));

        // inside .git/ — excluded
        assert!(!super::should_rebuild(
            &root.join(".git/hooks/post-commit"),
            &root
        ));

        // non-.rs extension — excluded
        assert!(!super::should_rebuild(&root.join("src/main.toml"), &root));

        // outside project root — excluded
        assert!(!super::should_rebuild(
            &PathBuf::from("/other/src/lib.rs"),
            &root
        ));
    }
}
