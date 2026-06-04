use std::sync::mpsc;

use crate::core::error::Result;
use crate::runtime::RuntimeRequest;

use super::super::state::AppState;
use super::super::worker::WorkerCmd;
use super::Command;

enum CommandAction {
    Quit,
    ShowHelp,
    ClearSession,
    ListSessions,
    ClearProjectSessions,
    Runtime(RuntimeRequest),
}

pub(crate) fn dispatch_command_runtime_request(
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

pub(crate) fn submit_to_app(
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

fn resolve_command(cmd: Command) -> CommandAction {
    match cmd {
        Command::Help => CommandAction::ShowHelp,
        Command::Quit => CommandAction::Quit,
        Command::Clear => CommandAction::ClearSession,
        Command::Approve => CommandAction::Runtime(RuntimeRequest::Approve),
        Command::Reject => CommandAction::Runtime(RuntimeRequest::Reject),
        Command::Last => CommandAction::Runtime(RuntimeRequest::QueryLast),
        Command::Anchors => CommandAction::Runtime(RuntimeRequest::QueryAnchors),
        Command::History => CommandAction::Runtime(RuntimeRequest::QueryHistory),
        Command::Read(path) => CommandAction::Runtime(RuntimeRequest::ReadFile { path }),
        Command::Search(query) => CommandAction::Runtime(RuntimeRequest::SearchCode { query }),
        Command::Sessions => CommandAction::ListSessions,
        Command::SessionClear => CommandAction::ClearProjectSessions,
        Command::Undo => CommandAction::Runtime(RuntimeRequest::Undo),
        Command::ProvidersList => CommandAction::Runtime(RuntimeRequest::ProvidersList),
        Command::ProvidersUse(name) => {
            CommandAction::Runtime(RuntimeRequest::ProvidersUse { name })
        }
        Command::GitBranch => CommandAction::Runtime(RuntimeRequest::GitBranch),
        Command::GitStatus => CommandAction::Runtime(RuntimeRequest::GitStatus),
        Command::GitDiff => CommandAction::Runtime(RuntimeRequest::GitDiff),
        Command::GitLog => CommandAction::Runtime(RuntimeRequest::GitLog),
        Command::BranchList => CommandAction::Runtime(RuntimeRequest::GitBranch),
        Command::BranchCreate(name) => CommandAction::Runtime(RuntimeRequest::BranchCreate {
            name,
            start_point: None,
        }),
        Command::BranchSwitch(name) => {
            CommandAction::Runtime(RuntimeRequest::BranchSwitch { name })
        }
        Command::Commit { message } => CommandAction::Runtime(RuntimeRequest::Commit { message }),
        Command::Diff(mode) => CommandAction::Runtime(RuntimeRequest::Diff { mode }),
        Command::Ls(path) => CommandAction::Runtime(RuntimeRequest::ListDir { path }),
        Command::LspStatus => CommandAction::Runtime(RuntimeRequest::LspStatus),
        Command::IndexBuild { large } => {
            CommandAction::Runtime(RuntimeRequest::IndexBuild { large })
        }
        Command::IndexStatus => CommandAction::Runtime(RuntimeRequest::IndexStatus),
        Command::IndexEmbed => CommandAction::Runtime(RuntimeRequest::IndexEmbed),
        Command::ContextStats => CommandAction::Runtime(RuntimeRequest::ContextStats),
        Command::Compact => CommandAction::Runtime(RuntimeRequest::Compact),
        Command::PromptPhysics(enabled) => {
            CommandAction::Runtime(RuntimeRequest::PromptPhysicsToggle { enabled })
        }
        Command::VerifyMutation(command) => {
            CommandAction::Runtime(RuntimeRequest::VerifyMutationToggle { command })
        }
        Command::TransactionStatus => CommandAction::Runtime(RuntimeRequest::TransactionStatus),
        Command::Ability(name) => CommandAction::Runtime(RuntimeRequest::AbilityToggle { name }),
        Command::Skill(name) => CommandAction::Runtime(RuntimeRequest::SkillToggle { name }),
        Command::Fetch(url) => CommandAction::Runtime(RuntimeRequest::FetchUrl { url }),
        Command::PlanCreate(goal) => CommandAction::Runtime(RuntimeRequest::PlanCreate { goal }),
        Command::PlanApprove => CommandAction::Runtime(RuntimeRequest::PlanApprove),
        Command::PlanAbandon => CommandAction::Runtime(RuntimeRequest::PlanAbandon),
        Command::PlanStatus => CommandAction::Runtime(RuntimeRequest::PlanStatus),
        Command::TaskExecute { step } => {
            CommandAction::Runtime(RuntimeRequest::TaskExecute { step })
        }
        Command::TaskComplete { step, summary } => {
            CommandAction::Runtime(RuntimeRequest::TaskComplete { step, summary })
        }
        Command::TaskBlock { step, reason } => {
            CommandAction::Runtime(RuntimeRequest::TaskBlock { step, reason })
        }
        Command::TaskStatus => CommandAction::Runtime(RuntimeRequest::TaskStatus),
        Command::Agent { ability, target } => {
            CommandAction::Runtime(RuntimeRequest::AgentRun { ability, target })
        }
    }
}

pub(crate) fn handle_command(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    cmd: Command,
) -> Result<()> {
    match resolve_command(cmd) {
        CommandAction::ShowHelp => {
            state.add_system_message(
                "Commands:\n\n  Navigation\n    /read <path>          read a file\n    /search <query>       search code\n    /fetch <url>          fetch a URL and return plain text\n    /last                 show last response\n    /anchors              show anchor state\n    /history              conversation history\n\n  Git\n    /git status           git status\n    /git diff             git diff\n    /git log              git log\n    /git branch           current branch\n    /diff                 show unstaged working tree changes\n    /diff last            show all changes since session start\n    /diff <ref>           show changes vs commit or branch\n\n  Branch\n    /branch list          list local branches\n    /branch <name>        create branch from HEAD (requires approval)\n    /branch switch <name> switch to existing branch (requires approval, resets session)\n\n  Commit\n    /commit               generate conventional commit message (requires approval)\n    /commit \"msg\"         commit staged changes with provided message (requires approval)\n\n  Session\n    /sessions             list project sessions\n    /session clear        delete sessions and start fresh\n    /clear                clear transcript history\n\n  Actions\n    /approve              confirm pending action\n    /reject               cancel pending action\n    /undo                 revert last mutation\n\n  Providers\n    /providers list       list available providers\n    /providers use <name> switch provider (session-only)\n\n  Index\n    /index status         symbol count and last build time\n    /index build          build symbol index\n    /index build --large  build without file-count guard\n\n  Runtime\n    /prompt-physics on|off|status  toggle prompt physics injection\n    /verify on|off|status          toggle post-mutation cargo check\n    /ability <name>        set reasoning posture (debug, review, refactor, investigate, explain)\n    /ability list          list available abilities\n    /ability off           clear active ability\n    /skill <name>          set response style (concise, thorough, educational, critical, creative)\n    /skill list            list available skills\n    /skill off             clear active skill\n    /agent review [target]       run review workflow (structured critique)\n    /agent investigate [target]  run investigate workflow (findings report)\n\n  General\n    /help                 show this message\n    /quit                 exit",
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
