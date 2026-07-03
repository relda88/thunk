pub(crate) mod dispatch;

use crate::core::config::{AllowedCommandTool, Config, InvestigationDepth};
use crate::runtime::{DiffMode, RuntimeRequest};

/// A parsed slash command entered by the user.
/// Command parsing is a pure transformation — no runtime calls, no side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Quit,
    Clear,
    Approve,
    Reject,
    Last,
    Anchors,
    History,
    Read(String),
    Search(String),
    Sessions,
    SessionClear,
    Undo,
    ProvidersList,
    ProvidersUse(String),
    GitBranch,
    GitStatus,
    GitDiff,
    GitLog,
    BranchCreate(String),
    BranchList,
    BranchSwitch(String),
    Commit {
        message: Option<String>,
    },
    Diff(DiffMode),
    Ls(String),
    LspStatus,
    McpList,
    McpRefresh,
    IndexBuild {
        large: bool,
    },
    IndexStatus,
    IndexEmbed,
    ContextStats,
    Compact,
    PromptPhysics(Option<bool>),
    VerifyMutation(Option<String>),
    TransactionStatus,
    Ability(Option<String>),
    Skill(Option<String>),
    Fetch(String),
    PlanCreate(String),
    PlanApprove,
    PlanAbandon,
    PlanStatus,
    TaskExecute {
        step: usize,
    },
    TaskComplete {
        step: usize,
        summary: Option<String>,
    },
    TaskBlock {
        step: usize,
        reason: Option<String>,
    },
    TaskStatus,
    Agent {
        ability: String,
        target: Option<String>,
    },
    /// /depth shallow|normal|deep — session-scoped investigation depth toggle.
    /// None = query current status.
    Depth(Option<InvestigationDepth>),
    /// /retrieval log [n] — show last N retrieval turn summaries (default 10).
    RetrievalLog {
        n: Option<usize>,
    },
    /// /constrain on|off — session-scoped constrained output toggle.
    /// None = query current status.
    ConstrainedOutput(Option<bool>),
    /// /compress on|off — session-scoped ability prompt compression toggle.
    /// When enabled, injects only reasoning_effect instead of full invariants + specification.
    /// None = query current status.
    Compress(Option<bool>),
    /// Decompose a refactor goal into an edit sequence. None = bare /refactor with no goal.
    Refactor(Option<String>),
    /// /refactor approve — approve the latest sequence and begin executing steps.
    RefactorApprove,
    /// /refactor abort — abort the active sequence and mark it Failed.
    RefactorAbort,
    /// /refactor status — show active sequence goal and step progress.
    RefactorStatus,
    /// /remember <fact> — propose a memory fact for user approval.
    Remember(String),
    /// /memory — list all stored memory facts.
    Memory,
    /// /forget <id> — propose deletion of the memory fact with the given id.
    Forget(i64),
    /// /reflect — run a generation pass over recent conversation and propose extracted facts.
    Reflect,
    /// /exec on|off|status — session-scoped Tier-3 exec mode toggle.
    /// Some(true) enables, Some(false) disables, None queries current status.
    Exec(Option<bool>),
    /// /dnd on|off|status — session-scoped do-not-disturb mode toggle.
    /// Some(true) enables, Some(false) disables, None queries current status.
    Dnd(Option<bool>),
}

/// A parse-level error for slash commands. Returned when input begins with `/`
/// but is structurally invalid — not forwarded to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The slash prefix was present but the command name is not recognized.
    UnknownCommand,
    /// The command requires an argument that was not provided.
    MissingArgument { command: &'static str },
}

impl ParseError {
    pub fn user_message(&self) -> String {
        match self {
            Self::UnknownCommand => "unknown command".to_string(),
            Self::MissingArgument { command } => format!("{command}: argument required"),
        }
    }
}

/// Parses user input into a command result.
///
/// - `None`          — no `/` prefix: route to runtime as a normal prompt
/// - `Some(Ok(cmd))` — valid recognized command: execute
/// - `Some(Err(e))`  — slash command attempted but invalid: surface error, do not route to runtime
///
/// Parsing is pure — no runtime calls, no side effects.
pub fn parse(input: &str) -> Option<Result<Command, ParseError>> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or(trimmed);
    let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());

    match name {
        "/help" => Some(Ok(Command::Help)),
        "/quit" | "/exit" => Some(Ok(Command::Quit)),
        "/clear" => Some(Ok(Command::Clear)),
        "/approve" => Some(Ok(Command::Approve)),
        "/reject" => Some(Ok(Command::Reject)),
        "/last" => Some(Ok(Command::Last)),
        "/anchors" => Some(Ok(Command::Anchors)),
        "/history" => Some(Ok(Command::History)),
        "/read" => match arg {
            Some(path) => Some(Ok(Command::Read(path.to_string()))),
            None => Some(Err(ParseError::MissingArgument { command: "/read" })),
        },
        "/search" => match arg {
            Some(query) => Some(Ok(Command::Search(query.to_string()))),
            None => Some(Err(ParseError::MissingArgument { command: "/search" })),
        },
        "/undo" => Some(Ok(Command::Undo)),
        "/providers" => match arg {
            Some("list") => Some(Ok(Command::ProvidersList)),
            Some(rest) if rest.starts_with("use ") => {
                let name = rest["use ".len()..].trim().to_string();
                if name.is_empty() {
                    Some(Err(ParseError::UnknownCommand))
                } else {
                    Some(Ok(Command::ProvidersUse(name)))
                }
            }
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/git" => match arg {
            Some("branch") => Some(Ok(Command::GitBranch)),
            Some("status") => Some(Ok(Command::GitStatus)),
            Some("diff") => Some(Ok(Command::GitDiff)),
            Some("log") => Some(Ok(Command::GitLog)),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        // /branch list           — list local branches (reuses git_branch tool)
        // /branch <name>         — create branch from HEAD
        // /branch switch <name>  — switch to existing branch
        // Note: a branch literally named "list" cannot be created via /branch — use git directly.
        "/commit" => Some(Ok(Command::Commit {
            message: arg.map(str::to_string),
        })),
        "/diff" => match arg {
            None => Some(Ok(Command::Diff(DiffMode::WorkingTree))),
            Some("last") => Some(Ok(Command::Diff(DiffMode::SessionStart))),
            Some(ref_) => Some(Ok(Command::Diff(DiffMode::Ref(ref_.to_string())))),
        },
        "/branch" => match arg {
            None | Some("list") => Some(Ok(Command::BranchList)),
            Some("switch") => Some(Err(ParseError::MissingArgument {
                command: "/branch switch",
            })),
            Some(rest) if rest.starts_with("switch ") => {
                let name = rest["switch ".len()..].trim().to_string();
                if name.is_empty() {
                    Some(Err(ParseError::MissingArgument {
                        command: "/branch switch",
                    }))
                } else {
                    Some(Ok(Command::BranchSwitch(name)))
                }
            }
            Some(name) if !name.is_empty() => Some(Ok(Command::BranchCreate(name.to_string()))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/lsp" => match arg {
            Some("status") => Some(Ok(Command::LspStatus)),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/mcp" => match arg {
            None => Some(Ok(Command::McpList)),
            Some("list") => Some(Ok(Command::McpList)),
            Some("refresh") => Some(Ok(Command::McpRefresh)),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/index" => match arg {
            Some("status") => Some(Ok(Command::IndexStatus)),
            Some("build") => Some(Ok(Command::IndexBuild { large: false })),
            Some("build --large") => Some(Ok(Command::IndexBuild { large: true })),
            Some("embed") => Some(Ok(Command::IndexEmbed)),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/context" => match arg {
            Some("stats") => Some(Ok(Command::ContextStats)),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/compact" => Some(Ok(Command::Compact)),
        "/prompt-physics" => match arg {
            Some("on") => Some(Ok(Command::PromptPhysics(Some(true)))),
            Some("off") => Some(Ok(Command::PromptPhysics(Some(false)))),
            Some("status") | None => Some(Ok(Command::PromptPhysics(None))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/verify" => match arg {
            Some("off") => Some(Ok(Command::VerifyMutation(Some("off".to_string())))),
            Some("status") | None => Some(Ok(Command::VerifyMutation(None))),
            Some(cmd) => Some(Ok(Command::VerifyMutation(Some(cmd.to_string())))),
        },
        "/transaction" => Some(Ok(Command::TransactionStatus)),
        "/ability" => Some(Ok(Command::Ability(arg.map(str::to_string)))),
        "/skill" => Some(Ok(Command::Skill(arg.map(str::to_string)))),
        "/fetch" => match arg {
            Some(url) => Some(Ok(Command::Fetch(url.to_string()))),
            None => Some(Err(ParseError::MissingArgument { command: "/fetch" })),
        },
        "/plan" => match arg {
            None | Some("status") => Some(Ok(Command::PlanStatus)),
            Some("approve") => Some(Ok(Command::PlanApprove)),
            Some("abandon") => Some(Ok(Command::PlanAbandon)),
            Some(goal) if !goal.is_empty() => Some(Ok(Command::PlanCreate(goal.to_string()))),
            _ => Some(Err(ParseError::MissingArgument { command: "/plan" })),
        },
        "/task" => match arg {
            None | Some("status") => Some(Ok(Command::TaskStatus)),
            Some(rest) if rest.starts_with("complete") => {
                let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                match parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    Some(step) => Some(Ok(Command::TaskComplete {
                        step,
                        summary: parts.get(2).map(|s| s.to_string()),
                    })),
                    None => Some(Err(ParseError::MissingArgument {
                        command: "/task complete",
                    })),
                }
            }
            Some(rest) if rest.starts_with("block") => {
                let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                match parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    Some(step) => Some(Ok(Command::TaskBlock {
                        step,
                        reason: parts.get(2).map(|s| s.to_string()),
                    })),
                    None => Some(Err(ParseError::MissingArgument {
                        command: "/task block",
                    })),
                }
            }
            Some(rest) => match rest.trim().parse::<usize>() {
                Ok(step) => Some(Ok(Command::TaskExecute { step })),
                Err(_) => Some(Err(ParseError::MissingArgument { command: "/task" })),
            },
        },
        "/agent" => match arg {
            None => Some(Err(ParseError::MissingArgument { command: "/agent" })),
            Some(rest) => {
                let mut parts = rest.splitn(2, ' ');
                let ability = parts.next().unwrap_or("").to_string();
                let target = parts.next().map(|s| s.to_string());
                if ability.is_empty() {
                    Some(Err(ParseError::MissingArgument { command: "/agent" }))
                } else {
                    Some(Ok(Command::Agent { ability, target }))
                }
            }
        },
        "/depth" => match arg {
            Some("shallow") => Some(Ok(Command::Depth(Some(InvestigationDepth::Shallow)))),
            Some("normal") => Some(Ok(Command::Depth(Some(InvestigationDepth::Normal)))),
            Some("deep") => Some(Ok(Command::Depth(Some(InvestigationDepth::Deep)))),
            None | Some("status") => Some(Ok(Command::Depth(None))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/retrieval" => match arg {
            None | Some("log") => Some(Ok(Command::RetrievalLog { n: None })),
            Some(rest) if rest.starts_with("log") => {
                let n = rest["log".len()..].trim().parse::<usize>().ok();
                Some(Ok(Command::RetrievalLog { n }))
            }
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/constrain" => match arg {
            Some("on") => Some(Ok(Command::ConstrainedOutput(Some(true)))),
            Some("off") => Some(Ok(Command::ConstrainedOutput(Some(false)))),
            Some("status") | None => Some(Ok(Command::ConstrainedOutput(None))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/compress" => match arg {
            Some("on") => Some(Ok(Command::Compress(Some(true)))),
            Some("off") => Some(Ok(Command::Compress(Some(false)))),
            Some("status") | None => Some(Ok(Command::Compress(None))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/refactor" => match arg {
            Some("approve") => Some(Ok(Command::RefactorApprove)),
            Some("abort") => Some(Ok(Command::RefactorAbort)),
            Some("status") => Some(Ok(Command::RefactorStatus)),
            arg => {
                let goal = arg.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                Some(Ok(Command::Refactor(goal)))
            }
        },
        "/remember" => match arg {
            Some(fact) if !fact.trim().is_empty() => {
                Some(Ok(Command::Remember(fact.trim().to_string())))
            }
            _ => Some(Err(ParseError::MissingArgument {
                command: "/remember",
            })),
        },
        "/memory" => Some(Ok(Command::Memory)),
        "/forget" => match arg {
            Some(raw) => match raw.trim().parse::<i64>() {
                Ok(id) => Some(Ok(Command::Forget(id))),
                Err(_) => Some(Err(ParseError::UnknownCommand)),
            },
            None => Some(Err(ParseError::MissingArgument { command: "/forget" })),
        },
        "/reflect" => Some(Ok(Command::Reflect)),
        "/exec" => match arg {
            Some("on") => Some(Ok(Command::Exec(Some(true)))),
            Some("off") => Some(Ok(Command::Exec(Some(false)))),
            Some("status") | None => Some(Ok(Command::Exec(None))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/dnd" => match arg {
            Some("on") => Some(Ok(Command::Dnd(Some(true)))),
            Some("off") => Some(Ok(Command::Dnd(Some(false)))),
            Some("status") | None => Some(Ok(Command::Dnd(None))),
            _ => Some(Err(ParseError::UnknownCommand)),
        },
        "/ls" => Some(Ok(Command::Ls(arg.unwrap_or(".").to_string()))),
        "/sessions" => Some(Ok(Command::Sessions)),
        "/session" => match arg {
            Some("clear") => Some(Ok(Command::SessionClear)),
            Some(_) => Some(Err(ParseError::UnknownCommand)),
            None => Some(Err(ParseError::MissingArgument {
                command: "/session",
            })),
        },
        _ => Some(Err(ParseError::UnknownCommand)),
    }
}

/// Resolved action from a parsed slash command.
#[derive(Debug)]
pub(crate) enum CommandAction {
    Quit,
    ShowHelp,
    ClearSession,
    ListSessions,
    ClearProjectSessions,
    Runtime(RuntimeRequest),
}

/// Maps a parsed `Command` to the action the runtime/TUI should take.
pub(crate) fn resolve_command(cmd: Command) -> CommandAction {
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
        Command::McpList => CommandAction::Runtime(RuntimeRequest::McpList),
        Command::McpRefresh => CommandAction::Runtime(RuntimeRequest::McpRefresh),
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
        Command::Depth(depth) => {
            CommandAction::Runtime(RuntimeRequest::InvestigationDepthToggle { depth })
        }
        Command::RetrievalLog { n } => CommandAction::Runtime(RuntimeRequest::RetrievalLog { n }),
        Command::ConstrainedOutput(enabled) => {
            CommandAction::Runtime(RuntimeRequest::ConstrainedOutputToggle { enabled })
        }
        Command::Compress(enabled) => {
            CommandAction::Runtime(RuntimeRequest::CompressToggle { enabled })
        }
        Command::Refactor(goal) => {
            CommandAction::Runtime(RuntimeRequest::Refactor { target: goal })
        }
        Command::RefactorApprove => CommandAction::Runtime(RuntimeRequest::SequenceApprove),
        Command::RefactorAbort => CommandAction::Runtime(RuntimeRequest::SequenceAbort),
        Command::RefactorStatus => CommandAction::Runtime(RuntimeRequest::SequenceStatus),
        Command::Remember(fact) => CommandAction::Runtime(RuntimeRequest::Remember { fact }),
        Command::Memory => CommandAction::Runtime(RuntimeRequest::MemoryList),
        Command::Forget(id) => CommandAction::Runtime(RuntimeRequest::MemoryForget { id }),
        Command::Reflect => CommandAction::Runtime(RuntimeRequest::Reflect),
        Command::Exec(enabled) => CommandAction::Runtime(RuntimeRequest::ExecToggle { enabled }),
        Command::Dnd(enabled) => CommandAction::Runtime(RuntimeRequest::DndToggle { enabled }),
    }
}

pub(crate) fn help_text() -> &'static str {
    "Commands:\n\n  Navigation\n    /read <path>          read a file\n    /search <query>       search code\n    /fetch <url>          fetch a URL and return plain text\n    /last                 show last response\n    /anchors              show anchor state\n    /history              conversation history\n\n  Git\n    /git status           git status\n    /git diff             git diff\n    /git log              git log\n    /git branch           current branch\n    /diff                 show unstaged working tree changes\n    /diff last            show all changes since session start\n    /diff <ref>           show changes vs commit or branch\n\n  Branch\n    /branch list          list local branches\n    /branch <name>        create branch from HEAD (requires approval)\n    /branch switch <name> switch to existing branch (requires approval, resets session)\n\n  Commit\n    /commit               generate conventional commit message (requires approval)\n    /commit \"msg\"         commit staged changes with provided message (requires approval)\n\n  Session\n    /sessions             list project sessions\n    /session clear        delete sessions and start fresh\n    /clear                clear transcript history\n\n  Actions\n    /approve              confirm pending action\n    /reject               cancel pending action\n    /undo                 revert last mutation\n\n  Providers\n    /providers list       list available providers\n    /providers use <name> switch provider (session-only)\n\n  Index\n    /index status         symbol count and last build time\n    /index build          build symbol index\n    /index build --large  build without file-count guard\n\n  MCP\n    /mcp                  list configured MCP servers and status\n    /mcp refresh          re-run MCP tool discovery\n\n  Runtime\n    /prompt-physics on|off|status  toggle prompt physics injection\n    /verify on|off|status          toggle post-mutation cargo check\n    /ability <name>        set reasoning posture (debug, review, refactor, investigate, explain)\n    /ability list          list available abilities\n    /ability off           clear active ability\n    /skill <name>          set response style (concise, thorough, educational, critical, creative)\n    /skill list            list available skills\n    /skill off             clear active skill\n    /agent review [target]       run review workflow (structured critique)\n    /agent investigate [target]  run investigate workflow (findings report)\n    /depth shallow|normal|deep   set investigation depth (session-only)\n    /retrieval log [n]           show last N retrieval turn summaries (default 10)\n\n  General\n    /help                 show this message\n    /quit                 exit"
}

/// Returns the complete set of first-level slash command tokens for Tab autocomplete.
/// Must stay adjacent to parse() so additions to one are reflected in the other.
pub(crate) fn autocomplete_names() -> &'static [&'static str] {
    &[
        "/ability",
        "/agent",
        "/anchors",
        "/approve",
        "/branch",
        "/clear",
        "/commit",
        "/compact",
        "/compress",
        "/constrain",
        "/diff",
        "/context",
        "/depth",
        "/dnd",
        "/exec",
        "/exit",
        "/fetch",
        "/git",
        "/help",
        "/history",
        "/index",
        "/last",
        "/ls",
        "/lsp",
        "/mcp",
        "/mcp refresh",
        "/plan",
        "/prompt-physics",
        "/providers",
        "/quit",
        "/read",
        "/refactor",
        "/refactor approve",
        "/refactor abort",
        "/refactor status",
        "/reject",
        "/forget",
        "/memory",
        "/reflect",
        "/remember",
        "/retrieval",
        "/search",
        "/session",
        "/sessions",
        "/skill",
        "/task",
        "/transaction",
        "/undo",
        "/verify",
    ]
}

pub(crate) struct LauncherCommand {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

/// Returns the full command list for the Ctrl+K launcher.
/// Must stay adjacent to autocomplete_names() so additions to one are reflected in the other.
pub(crate) fn launcher_commands() -> &'static [LauncherCommand] {
    &[
        LauncherCommand {
            name: "/ability",
            description: "set reasoning posture: debug, review, refactor, investigate, explain",
        },
        LauncherCommand {
            name: "/agent",
            description: "run ability-driven workflow: review or investigate [target]",
        },
        LauncherCommand {
            name: "/anchors",
            description: "show last-read file and search anchors",
        },
        LauncherCommand {
            name: "/approve",
            description: "approve a pending tool action",
        },
        LauncherCommand {
            name: "/branch",
            description: "create, switch, or list git branches",
        },
        LauncherCommand {
            name: "/clear",
            description: "clear the transcript",
        },
        LauncherCommand {
            name: "/commit",
            description: "generate and approve a conventional commit message",
        },
        LauncherCommand {
            name: "/compact",
            description: "summarize and compress conversation context",
        },
        LauncherCommand {
            name: "/depth",
            description: "set investigation depth: shallow, normal (default), deep",
        },
        LauncherCommand {
            name: "/retrieval",
            description: "show retrieval quality log: /retrieval log [n] (default 10)",
        },
        LauncherCommand {
            name: "/diff",
            description:
                "show diff: working tree, session delta (/diff last), or vs commit (/diff <ref>)",
        },
        LauncherCommand {
            name: "/context",
            description: "show context window usage stats",
        },
        LauncherCommand {
            name: "/dnd",
            description:
                "enable or disable do-not-disturb / proactive suggestions (/dnd on|off|status)",
        },
        LauncherCommand {
            name: "/exec",
            description: "enable or disable Tier-3 arbitrary shell execution (/exec on|off|status)",
        },
        LauncherCommand {
            name: "/exit",
            description: "quit the application",
        },
        LauncherCommand {
            name: "/fetch",
            description: "fetch a URL and return plain text content",
        },
        LauncherCommand {
            name: "/git",
            description: "run a git command (branch, status, diff, log)",
        },
        LauncherCommand {
            name: "/help",
            description: "list available commands",
        },
        LauncherCommand {
            name: "/history",
            description: "show recent input history",
        },
        LauncherCommand {
            name: "/index",
            description: "manage the symbol index (status, build, embed)",
        },
        LauncherCommand {
            name: "/last",
            description: "re-run the previous prompt",
        },
        LauncherCommand {
            name: "/ls",
            description: "list directory contents",
        },
        LauncherCommand {
            name: "/lsp",
            description: "show LSP server status",
        },
        LauncherCommand {
            name: "/mcp",
            description: "list configured MCP servers and status",
        },
        LauncherCommand {
            name: "/mcp refresh",
            description: "re-run MCP tool discovery",
        },
        LauncherCommand {
            name: "/plan",
            description: "create a structured plan from a goal (/plan <goal>)",
        },
        LauncherCommand {
            name: "/compress",
            description: "Toggle ability prompt compression (reasoning_effect only vs full block)",
        },
        LauncherCommand {
            name: "/refactor",
            description: "Decompose a refactor goal into an edit sequence",
        },
        LauncherCommand {
            name: "/refactor approve",
            description: "Approve the latest refactor sequence and begin executing steps",
        },
        LauncherCommand {
            name: "/refactor abort",
            description: "Abort the active refactor sequence",
        },
        LauncherCommand {
            name: "/refactor status",
            description: "Show active refactor sequence goal and step progress",
        },
        LauncherCommand {
            name: "/remember",
            description: "propose a fact for personal memory (/remember <fact>)",
        },
        LauncherCommand {
            name: "/memory",
            description: "list all stored memory facts",
        },
        LauncherCommand {
            name: "/forget",
            description: "propose deletion of a stored memory fact (/forget <id>)",
        },
        LauncherCommand {
            name: "/reflect",
            description: "extract and propose memory facts from recent conversation",
        },
        LauncherCommand {
            name: "/constrain",
            description: "enable, disable, or check constrained output (grammar/JSON mode)",
        },
        LauncherCommand {
            name: "/prompt-physics",
            description: "enable, disable, or check prompt physics",
        },
        LauncherCommand {
            name: "/providers",
            description: "list or switch AI providers",
        },
        LauncherCommand {
            name: "/quit",
            description: "quit the application",
        },
        LauncherCommand {
            name: "/read",
            description: "load a file into context",
        },
        LauncherCommand {
            name: "/reject",
            description: "reject a pending tool action",
        },
        LauncherCommand {
            name: "/search",
            description: "search code for a pattern",
        },
        LauncherCommand {
            name: "/session",
            description: "manage current session (clear)",
        },
        LauncherCommand {
            name: "/sessions",
            description: "list saved sessions",
        },
        LauncherCommand {
            name: "/skill",
            description: "set response style: concise, thorough, educational, critical, creative",
        },
        LauncherCommand {
            name: "/task",
            description:
                "execute a plan step (/task <n>), mark complete (/task complete <n>), or blocked",
        },
        LauncherCommand {
            name: "/transaction",
            description: "show pending transaction state",
        },
        LauncherCommand {
            name: "/undo",
            description: "undo the last assistant action",
        },
        LauncherCommand {
            name: "/verify",
            description: "enable, disable, or check post-mutation cargo check",
        },
    ]
}

/// Resolves a raw input string against the custom command definitions in config.
///
/// Returns:
/// - `None`           — no custom command with this name; caller shows "unknown command"
/// - `Some(Err(msg))` — command found but argument is missing
/// - `Some(Ok(req))`  — resolved to a RuntimeRequest ready for dispatch
pub(crate) fn resolve_custom_command(
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
    use super::*;

    #[test]
    fn parses_help() {
        assert_eq!(parse("/help"), Some(Ok(Command::Help)));
    }

    #[test]
    fn parses_quit_aliases() {
        assert_eq!(parse("/quit"), Some(Ok(Command::Quit)));
        assert_eq!(parse("/exit"), Some(Ok(Command::Quit)));
    }

    #[test]
    fn parses_clear() {
        assert_eq!(parse("/clear"), Some(Ok(Command::Clear)));
    }

    #[test]
    fn parses_approve() {
        assert_eq!(parse("/approve"), Some(Ok(Command::Approve)));
    }

    #[test]
    fn parses_reject() {
        assert_eq!(parse("/reject"), Some(Ok(Command::Reject)));
    }

    #[test]
    fn parses_last() {
        assert_eq!(parse("/last"), Some(Ok(Command::Last)));
    }

    #[test]
    fn parses_anchors() {
        assert_eq!(parse("/anchors"), Some(Ok(Command::Anchors)));
    }

    #[test]
    fn parses_history() {
        assert_eq!(parse("/history"), Some(Ok(Command::History)));
    }

    #[test]
    fn ignores_whitespace() {
        assert_eq!(parse("  /help  "), Some(Ok(Command::Help)));
    }

    #[test]
    fn extra_args_on_no_arg_command_are_ignored() {
        assert_eq!(parse("/help extra stuff"), Some(Ok(Command::Help)));
        assert_eq!(parse("/history some arg"), Some(Ok(Command::History)));
    }

    #[test]
    fn non_command_returns_none() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("how do I fix this bug?"), None);
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn unknown_slash_command_returns_error() {
        assert_eq!(parse("/unknown"), Some(Err(ParseError::UnknownCommand)));
        assert_eq!(parse("/foo"), Some(Err(ParseError::UnknownCommand)));
    }

    #[test]
    fn unknown_command_error_message() {
        let e = ParseError::UnknownCommand;
        assert_eq!(e.user_message(), "unknown command");
    }

    #[test]
    fn missing_argument_error_message() {
        let e = ParseError::MissingArgument { command: "/read" };
        assert_eq!(e.user_message(), "/read: argument required");
    }

    #[test]
    fn parses_read_with_path() {
        assert_eq!(
            parse("/read src/main.rs"),
            Some(Ok(Command::Read("src/main.rs".to_string())))
        );
    }

    #[test]
    fn parses_search_with_query() {
        assert_eq!(
            parse("/search fn handle"),
            Some(Ok(Command::Search("fn handle".to_string())))
        );
    }

    #[test]
    fn parses_sessions() {
        assert_eq!(parse("/sessions"), Some(Ok(Command::Sessions)));
    }

    #[test]
    fn parses_session_clear() {
        assert_eq!(parse("/session clear"), Some(Ok(Command::SessionClear)));
    }

    #[test]
    fn read_without_arg_returns_missing_argument() {
        assert_eq!(
            parse("/read"),
            Some(Err(ParseError::MissingArgument { command: "/read" }))
        );
        assert_eq!(
            parse("/read   "),
            Some(Err(ParseError::MissingArgument { command: "/read" }))
        );
    }

    #[test]
    fn search_without_arg_returns_missing_argument() {
        assert_eq!(
            parse("/search"),
            Some(Err(ParseError::MissingArgument { command: "/search" }))
        );
        assert_eq!(
            parse("/search   "),
            Some(Err(ParseError::MissingArgument { command: "/search" }))
        );
    }

    #[test]
    fn session_without_subcommand_returns_missing_argument() {
        assert_eq!(
            parse("/session"),
            Some(Err(ParseError::MissingArgument {
                command: "/session"
            }))
        );
    }

    #[test]
    fn unknown_session_subcommand_returns_unknown_command() {
        assert_eq!(
            parse("/session list"),
            Some(Err(ParseError::UnknownCommand))
        );
    }

    #[test]
    fn parses_git_branch() {
        assert_eq!(parse("/git branch"), Some(Ok(Command::GitBranch)));
    }

    #[test]
    fn parses_git_status() {
        assert_eq!(parse("/git status"), Some(Ok(Command::GitStatus)));
    }

    #[test]
    fn parses_git_diff() {
        assert_eq!(parse("/git diff"), Some(Ok(Command::GitDiff)));
    }

    #[test]
    fn parses_git_log() {
        assert_eq!(parse("/git log"), Some(Ok(Command::GitLog)));
    }

    #[test]
    fn parses_branch_create() {
        assert_eq!(
            parse("/branch feat/x"),
            Some(Ok(Command::BranchCreate("feat/x".to_string())))
        );
    }

    #[test]
    fn parses_branch_list_explicit() {
        assert_eq!(parse("/branch list"), Some(Ok(Command::BranchList)));
    }

    #[test]
    fn parses_branch_bare_defaults_to_list() {
        assert_eq!(parse("/branch"), Some(Ok(Command::BranchList)));
    }

    #[test]
    fn parses_branch_switch() {
        assert_eq!(
            parse("/branch switch main"),
            Some(Ok(Command::BranchSwitch("main".to_string())))
        );
    }

    #[test]
    fn parses_branch_switch_with_slash_name() {
        assert_eq!(
            parse("/branch switch feat/my-feature"),
            Some(Ok(Command::BranchSwitch("feat/my-feature".to_string())))
        );
    }

    #[test]
    fn parses_branch_switch_missing_name_errors() {
        assert_eq!(
            parse("/branch switch"),
            Some(Err(ParseError::MissingArgument {
                command: "/branch switch"
            }))
        );
    }

    #[test]
    fn parses_ls_with_path() {
        assert_eq!(parse("/ls src/"), Some(Ok(Command::Ls("src/".to_string()))));
    }

    #[test]
    fn parses_lsp_status() {
        assert_eq!(parse("/lsp status"), Some(Ok(Command::LspStatus)));
    }

    #[test]
    fn parses_mcp_commands() {
        assert_eq!(parse("/mcp"), Some(Ok(Command::McpList)));
        assert_eq!(parse("/mcp list"), Some(Ok(Command::McpList)));
        assert_eq!(parse("/mcp refresh"), Some(Ok(Command::McpRefresh)));
        assert_eq!(parse("/mcp bogus"), Some(Err(ParseError::UnknownCommand)));
    }

    #[test]
    fn parses_ls_no_arg_defaults_to_dot() {
        assert_eq!(parse("/ls"), Some(Ok(Command::Ls(".".to_string()))));
        assert_eq!(parse("/ls   "), Some(Ok(Command::Ls(".".to_string()))));
    }

    #[test]
    fn parses_index_status() {
        assert_eq!(parse("/index status"), Some(Ok(Command::IndexStatus)));
    }

    #[test]
    fn parses_index_build() {
        assert_eq!(
            parse("/index build"),
            Some(Ok(Command::IndexBuild { large: false }))
        );
    }

    #[test]
    fn parses_index_build_large() {
        assert_eq!(
            parse("/index build --large"),
            Some(Ok(Command::IndexBuild { large: true }))
        );
    }

    #[test]
    fn index_unknown_subcommand_returns_unknown_command() {
        assert_eq!(parse("/index foo"), Some(Err(ParseError::UnknownCommand)));
    }

    #[test]
    fn parses_context_stats() {
        assert_eq!(parse("/context stats"), Some(Ok(Command::ContextStats)));
    }

    #[test]
    fn context_unknown_subcommand_returns_unknown_command() {
        assert_eq!(parse("/context"), Some(Err(ParseError::UnknownCommand)));
        assert_eq!(parse("/context foo"), Some(Err(ParseError::UnknownCommand)));
    }

    #[test]
    fn parses_compact() {
        assert_eq!(parse("/compact"), Some(Ok(Command::Compact)));
    }

    #[test]
    fn parses_prompt_physics_on() {
        assert_eq!(
            parse("/prompt-physics on"),
            Some(Ok(Command::PromptPhysics(Some(true))))
        );
    }

    #[test]
    fn parses_prompt_physics_off() {
        assert_eq!(
            parse("/prompt-physics off"),
            Some(Ok(Command::PromptPhysics(Some(false))))
        );
    }

    #[test]
    fn parses_prompt_physics_status() {
        assert_eq!(
            parse("/prompt-physics status"),
            Some(Ok(Command::PromptPhysics(None)))
        );
    }

    #[test]
    fn parses_prompt_physics_bare() {
        assert_eq!(
            parse("/prompt-physics"),
            Some(Ok(Command::PromptPhysics(None)))
        );
    }

    #[test]
    fn parses_verify_off() {
        assert_eq!(
            parse("/verify off"),
            Some(Ok(Command::VerifyMutation(Some("off".to_string()))))
        );
    }

    #[test]
    fn parses_verify_status() {
        assert_eq!(
            parse("/verify status"),
            Some(Ok(Command::VerifyMutation(None)))
        );
    }

    #[test]
    fn parses_verify_bare() {
        assert_eq!(parse("/verify"), Some(Ok(Command::VerifyMutation(None))));
    }

    #[test]
    fn parses_verify_command() {
        assert_eq!(
            parse("/verify cargo check"),
            Some(Ok(Command::VerifyMutation(Some("cargo check".to_string()))))
        );
    }

    #[test]
    fn parses_diff_bare() {
        assert_eq!(
            parse("/diff"),
            Some(Ok(Command::Diff(DiffMode::WorkingTree)))
        );
    }

    #[test]
    fn parses_diff_last() {
        assert_eq!(
            parse("/diff last"),
            Some(Ok(Command::Diff(DiffMode::SessionStart)))
        );
    }

    #[test]
    fn parses_diff_ref() {
        assert_eq!(
            parse("/diff main"),
            Some(Ok(Command::Diff(DiffMode::Ref("main".to_string()))))
        );
    }

    #[test]
    fn parses_ability_name() {
        assert_eq!(
            parse("/ability debug"),
            Some(Ok(Command::Ability(Some("debug".to_string()))))
        );
    }

    #[test]
    fn parses_ability_off() {
        assert_eq!(
            parse("/ability off"),
            Some(Ok(Command::Ability(Some("off".to_string()))))
        );
    }

    #[test]
    fn parses_ability_bare() {
        assert_eq!(parse("/ability"), Some(Ok(Command::Ability(None))));
    }

    #[test]
    fn parses_skill_name() {
        assert_eq!(
            parse("/skill concise"),
            Some(Ok(Command::Skill(Some("concise".to_string()))))
        );
    }

    #[test]
    fn parses_commit_bare() {
        assert_eq!(
            parse("/commit"),
            Some(Ok(Command::Commit { message: None }))
        );
    }

    #[test]
    fn parses_commit_with_message() {
        assert_eq!(
            parse("/commit feat: add foo"),
            Some(Ok(Command::Commit {
                message: Some("feat: add foo".to_string()),
            }))
        );
    }

    #[test]
    fn parses_fetch_url() {
        assert_eq!(
            parse("/fetch https://example.com"),
            Some(Ok(Command::Fetch("https://example.com".to_string())))
        );
    }

    #[test]
    fn parses_fetch_missing_arg() {
        assert_eq!(
            parse("/fetch"),
            Some(Err(ParseError::MissingArgument { command: "/fetch" }))
        );
    }

    #[test]
    fn parses_fetch_url_with_path() {
        assert_eq!(
            parse("/fetch https://example.com/path?q=1"),
            Some(Ok(Command::Fetch(
                "https://example.com/path?q=1".to_string()
            )))
        );
    }

    #[test]
    fn parses_plan_create() {
        assert_eq!(
            parse("/plan refactor auth"),
            Some(Ok(Command::PlanCreate("refactor auth".to_string())))
        );
    }

    #[test]
    fn parses_plan_status_explicit() {
        assert_eq!(parse("/plan status"), Some(Ok(Command::PlanStatus)));
    }

    #[test]
    fn parses_plan_bare_defaults_to_status() {
        assert_eq!(parse("/plan"), Some(Ok(Command::PlanStatus)));
    }

    #[test]
    fn parses_plan_approve() {
        assert_eq!(parse("/plan approve"), Some(Ok(Command::PlanApprove)));
    }

    #[test]
    fn parses_plan_abandon() {
        assert_eq!(parse("/plan abandon"), Some(Ok(Command::PlanAbandon)));
    }

    #[test]
    fn parses_constrain_on() {
        assert_eq!(
            parse("/constrain on"),
            Some(Ok(Command::ConstrainedOutput(Some(true))))
        );
    }

    #[test]
    fn parses_constrain_off() {
        assert_eq!(
            parse("/constrain off"),
            Some(Ok(Command::ConstrainedOutput(Some(false))))
        );
    }

    #[test]
    fn parses_constrain_status() {
        assert_eq!(
            parse("/constrain status"),
            Some(Ok(Command::ConstrainedOutput(None)))
        );
    }

    #[test]
    fn parses_constrain_bare() {
        assert_eq!(
            parse("/constrain"),
            Some(Ok(Command::ConstrainedOutput(None)))
        );
    }

    #[test]
    fn parses_compress_on() {
        assert_eq!(
            parse("/compress on"),
            Some(Ok(Command::Compress(Some(true))))
        );
    }

    #[test]
    fn parses_compress_off() {
        assert_eq!(
            parse("/compress off"),
            Some(Ok(Command::Compress(Some(false))))
        );
    }

    #[test]
    fn parses_compress_status() {
        assert_eq!(parse("/compress status"), Some(Ok(Command::Compress(None))));
    }

    #[test]
    fn parses_compress_bare() {
        assert_eq!(parse("/compress"), Some(Ok(Command::Compress(None))));
    }

    #[test]
    fn parses_refactor_with_goal() {
        assert_eq!(
            parse("/refactor extract error handling"),
            Some(Ok(Command::Refactor(Some(
                "extract error handling".to_string()
            ))))
        );
    }

    #[test]
    fn parses_refactor_bare() {
        assert_eq!(parse("/refactor"), Some(Ok(Command::Refactor(None))));
    }

    #[test]
    fn parses_refactor_approve() {
        assert_eq!(
            parse("/refactor approve"),
            Some(Ok(Command::RefactorApprove))
        );
    }

    #[test]
    fn parses_refactor_abort() {
        assert_eq!(parse("/refactor abort"), Some(Ok(Command::RefactorAbort)));
    }

    #[test]
    fn parses_refactor_status() {
        assert_eq!(parse("/refactor status"), Some(Ok(Command::RefactorStatus)));
    }

    #[test]
    fn parses_refactor_goal_not_confused_with_subcommands() {
        assert_eq!(
            parse("/refactor extract helpers"),
            Some(Ok(Command::Refactor(Some("extract helpers".to_string()))))
        );
    }

    #[test]
    fn exec_toggle_enables() {
        assert_eq!(parse("/exec on"), Some(Ok(Command::Exec(Some(true)))));
    }

    #[test]
    fn exec_toggle_disables() {
        assert_eq!(parse("/exec off"), Some(Ok(Command::Exec(Some(false)))));
    }

    #[test]
    fn exec_toggle_status() {
        assert_eq!(parse("/exec status"), Some(Ok(Command::Exec(None))));
        assert_eq!(parse("/exec"), Some(Ok(Command::Exec(None))));
    }

    #[test]
    fn dnd_toggle_enables() {
        assert_eq!(parse("/dnd on"), Some(Ok(Command::Dnd(Some(true)))));
    }

    #[test]
    fn dnd_toggle_disables() {
        assert_eq!(parse("/dnd off"), Some(Ok(Command::Dnd(Some(false)))));
    }
}
