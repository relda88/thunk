use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::config::{AllowedCommandTool, Config};
use crate::app::paths::AppPaths;
use crate::app::AppContext;
use crate::app::Result;
use crate::runtime::{RuntimeEvent, RuntimeRequest};
use crate::storage::session::SessionMeta;

use super::renderer::Renderer;
use super::state::{AppState, DirtySections};
use super::{commands, events, format};

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
        if state.show_activity {
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

#[derive(Debug)]
enum WorkerCmd {
    Handle(RuntimeRequest),
    Reset,
    ListSessions,
    ClearSessions,
}

enum WorkerReply {
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

fn run_worker(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorShape {
    SteadyBar,
    SteadyBlock,
    SteadyUnderScore,
    BlinkingBlock,
}

impl CursorShape {
    fn to_crossterm(self) -> SetCursorStyle {
        match self {
            CursorShape::SteadyBar => SetCursorStyle::SteadyBar,
            CursorShape::SteadyBlock => SetCursorStyle::SteadyBlock,
            CursorShape::SteadyUnderScore => SetCursorStyle::SteadyUnderScore,
            CursorShape::BlinkingBlock => SetCursorStyle::BlinkingBlock,
        }
    }
}

fn sync_terminal_affordances(
    state: &AppState,
    last_shape: &mut Option<CursorShape>,
    out: &mut io::Stdout,
) -> io::Result<()> {
    let shape = if state.pending_approval.is_some() {
        CursorShape::BlinkingBlock
    } else if state.is_reverse_search_active() {
        CursorShape::SteadyUnderScore
    } else if state.is_busy {
        CursorShape::SteadyBlock
    } else {
        CursorShape::SteadyBar
    };
    if *last_shape != Some(shape) {
        crossterm::queue!(out, shape.to_crossterm())?;
        *last_shape = Some(shape);
    }
    Ok(())
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

fn handle_key_event(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    config: &Config,
    key: KeyEvent,
) -> Result<()> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }
        (KeyCode::Enter, KeyModifiers::ALT) => state.insert_newline(),
        (KeyCode::Esc, _) if state.is_reverse_search_active() => state.cancel_reverse_search(),
        (KeyCode::Enter, _) if state.is_reverse_search_active() => state.accept_reverse_search(),
        (KeyCode::Backspace, _) if state.is_reverse_search_active() => {
            state.reverse_search_backspace()
        }
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT)
            if state.is_reverse_search_active() =>
        {
            state.reverse_search_push_char(c)
        }
        (KeyCode::Enter, _) => {
            if let Some(input) = state.submit_input() {
                match commands::parse(&input) {
                    None => submit_to_app(state, cmd_tx, input)?,
                    Some(Ok(cmd)) => handle_command(state, cmd_tx, cmd)?,
                    Some(Err(commands::ParseError::UnknownCommand)) => {
                        match resolve_custom_command(config, &input) {
                            None => state.add_system_message(
                                commands::ParseError::UnknownCommand.user_message(),
                            ),
                            Some(Err(msg)) => state.add_system_message(msg),
                            Some(Ok(req)) => dispatch_command_runtime_request(state, cmd_tx, req)?,
                        }
                    }
                    Some(Err(e)) => state.add_system_message(e.user_message()),
                }
            }
        }
        (KeyCode::Backspace, KeyModifiers::ALT) => state.delete_word_before(),
        (KeyCode::Backspace, _) => state.delete_char_before(),
        (KeyCode::Left, _) => state.cursor_left(),
        (KeyCode::Right, _) => state.cursor_right(),
        (KeyCode::Home, _) => state.cursor_home(),
        (KeyCode::End, _) => state.cursor_end(),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            if let Some(prompt) = &state.last_prompt {
                let path = std::env::temp_dir().join("thunk_last_prompt.txt");
                format::dump_prompt_to_file(&path, prompt);
                state.set_status(&format!("prompt dumped to {}", path.display()));
            } else {
                state.set_status("no prompt captured yet");
            }
        }
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => state.recall_previous_input(),
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if state.pending_approval.is_some() {
                dispatch_command_runtime_request(state, cmd_tx, RuntimeRequest::Reject)?;
            } else {
                state.recall_next_input();
            }
        }
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
            if state.pending_approval.is_some() {
                dispatch_command_runtime_request(state, cmd_tx, RuntimeRequest::Approve)?;
            }
        }
        (KeyCode::Up, _) => state.scroll_up(1),
        (KeyCode::Down, _) => state.scroll_down(1),
        (KeyCode::PageUp, _) => state.scroll_up(10),
        (KeyCode::PageDown, _) => state.scroll_down(10),
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => state.toggle_file_expand(),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => state.delete_word_before(),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => state.reverse_search_cycle(),
        (KeyCode::Char('['), KeyModifiers::ALT) => state.focus_prev_collapsible(),
        (KeyCode::Char(']'), KeyModifiers::ALT) => state.focus_next_collapsible(),
        (KeyCode::Char('o'), KeyModifiers::ALT) => state.toggle_collapse_focused(),
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => state.insert_char(c),
        _ => {}
    }

    Ok(())
}

fn dispatch_command_runtime_request(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    req: RuntimeRequest,
) -> Result<()> {
    if state.is_busy {
        return Ok(());
    }
    state.is_busy = true;
    let _ = cmd_tx.send(WorkerCmd::Handle(req));
    Ok(())
}

fn submit_to_app(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    prompt: String,
) -> Result<()> {
    if state.is_busy {
        return Ok(());
    }
    state.add_user_message(prompt.clone());
    state.is_busy = true;
    let _ = cmd_tx.send(WorkerCmd::Handle(RuntimeRequest::Submit { text: prompt }));
    Ok(())
}

enum CommandAction {
    Quit,
    ShowHelp,
    ClearSession,
    ListSessions,
    ClearProjectSessions,
    Runtime(RuntimeRequest),
}

fn resolve_command(cmd: commands::Command) -> CommandAction {
    match cmd {
        commands::Command::Help => CommandAction::ShowHelp,
        commands::Command::Quit => CommandAction::Quit,
        commands::Command::Clear => CommandAction::ClearSession,
        commands::Command::Approve => CommandAction::Runtime(RuntimeRequest::Approve),
        commands::Command::Reject => CommandAction::Runtime(RuntimeRequest::Reject),
        commands::Command::Last => CommandAction::Runtime(RuntimeRequest::QueryLast),
        commands::Command::Anchors => CommandAction::Runtime(RuntimeRequest::QueryAnchors),
        commands::Command::History => CommandAction::Runtime(RuntimeRequest::QueryHistory),
        commands::Command::Read(path) => CommandAction::Runtime(RuntimeRequest::ReadFile { path }),
        commands::Command::Search(query) => {
            CommandAction::Runtime(RuntimeRequest::SearchCode { query })
        }
        commands::Command::Sessions => CommandAction::ListSessions,
        commands::Command::SessionClear => CommandAction::ClearProjectSessions,
        commands::Command::Undo => CommandAction::Runtime(RuntimeRequest::Undo),
        commands::Command::ProvidersList => CommandAction::Runtime(RuntimeRequest::ProvidersList),
        commands::Command::ProvidersUse(name) => {
            CommandAction::Runtime(RuntimeRequest::ProvidersUse { name })
        }
        commands::Command::GitBranch => CommandAction::Runtime(RuntimeRequest::GitBranch),
        commands::Command::GitStatus => CommandAction::Runtime(RuntimeRequest::GitStatus),
        commands::Command::GitDiff => CommandAction::Runtime(RuntimeRequest::GitDiff),
        commands::Command::GitLog => CommandAction::Runtime(RuntimeRequest::GitLog),
        commands::Command::Ls(path) => CommandAction::Runtime(RuntimeRequest::ListDir { path }),
        commands::Command::LspStatus => CommandAction::Runtime(RuntimeRequest::LspStatus),
        commands::Command::IndexBuild { large } => {
            CommandAction::Runtime(RuntimeRequest::IndexBuild { large })
        }
        commands::Command::IndexStatus => CommandAction::Runtime(RuntimeRequest::IndexStatus),
        commands::Command::ContextStats => CommandAction::Runtime(RuntimeRequest::ContextStats),
        commands::Command::Compact => CommandAction::Runtime(RuntimeRequest::Compact),
    }
}

fn handle_command(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    cmd: commands::Command,
) -> Result<()> {
    match resolve_command(cmd) {
        CommandAction::ShowHelp => {
            state.add_system_message(
                "Commands:\n\n  Navigation\n    /read <path>          read a file\n    /search <query>       search code\n    /last                 show last response\n    /anchors              show anchor state\n    /history              conversation history\n\n  Git\n    /git status           git status\n    /git diff             git diff\n    /git log              git log\n    /git branch           current branch\n\n  Session\n    /sessions             list project sessions\n    /session clear        delete sessions and start fresh\n    /clear                clear transcript history\n\n  Actions\n    /approve              confirm pending action\n    /reject               cancel pending action\n    /undo                 revert last mutation\n\n  Providers\n    /providers list       list available providers\n    /providers use <name> switch provider (session-only)\n\n  Index\n    /index status         symbol count and last build time\n    /index build          build symbol index\n    /index build --large  build without file-count guard\n\n  General\n    /help                 show this message\n    /quit                 exit",
            );
        }
        CommandAction::Quit => {
            state.should_quit = true;
        }
        CommandAction::ClearSession => {
            if state.is_busy {
                return Ok(());
            }
            state.clear_messages();
            state.is_busy = true;
            let _ = cmd_tx.send(WorkerCmd::Reset);
        }
        CommandAction::ListSessions => {
            if state.is_busy {
                return Ok(());
            }
            state.is_busy = true;
            let _ = cmd_tx.send(WorkerCmd::ListSessions);
        }
        CommandAction::ClearProjectSessions => {
            if state.is_busy {
                return Ok(());
            }
            state.clear_messages();
            state.is_busy = true;
            let _ = cmd_tx.send(WorkerCmd::ClearSessions);
        }
        CommandAction::Runtime(req) => {
            dispatch_command_runtime_request(state, cmd_tx, req)?;
        }
    }
    Ok(())
}

/// Resolves a raw input string against the custom command definitions in config.
///
/// Returns:
/// - `None`           — no custom command with this name; caller shows "unknown command"
/// - `Some(Err(msg))` — command found but argument is missing
/// - `Some(Ok(req))`  — resolved to a RuntimeRequest ready for dispatch
fn resolve_custom_command(
    config: &Config,
    input: &str,
) -> Option<std::result::Result<RuntimeRequest, String>> {
    let trimmed = input.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let slash_name = parts.next()?;
    let name = slash_name.strip_prefix('/')?;
    let def = config.commands.get(name)?;

    let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());
    let arg_str = match arg {
        Some(a) => a.to_string(),
        None => return Some(Err(format!("/{name}: argument required"))),
    };

    let value = def.template.replace("{input}", &arg_str);
    let req = match def.tool {
        AllowedCommandTool::ReadFile => RuntimeRequest::ReadFile { path: value },
        AllowedCommandTool::SearchCode => RuntimeRequest::SearchCode { query: value },
    };
    Some(Ok(req))
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

    use super::{handle_key_event, WorkerCmd};
    use crate::tui::state::{AppState, ApprovalRisk, PendingApprovalState};

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
                config_file: root_dir.path().join("config.toml"),
                data_dir: root_dir.path().join("data"),
                logs_dir: root_dir.path().join("logs"),
                session_db: root_dir.path().join("data").join("sessions.db"),
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
            evidence: vec![],
            preview: vec![],
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
            evidence: vec![],
            preview: vec![],
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
            evidence: vec![],
            preview: vec![],
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
}
