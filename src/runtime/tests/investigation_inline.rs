#[cfg(test)]
mod tests {
    use crate::runtime::investigation::investigation::*;

    #[test]
    fn looks_like_import_accepts_simple_import() {
        assert!(looks_like_import("import logging"));
        assert!(looks_like_import("import os, sys"));
        assert!(looks_like_import("  import logging"));
    }

    #[test]
    fn looks_like_import_accepts_from_import() {
        assert!(looks_like_import("from models.enums import TaskStatus"));
        assert!(looks_like_import("from . import utils"));
        assert!(looks_like_import("  from models.enums import TaskStatus"));
    }

    #[test]
    fn looks_like_import_rejects_usage_lines() {
        assert!(!looks_like_import(
            "if task.status == TaskStatus.TODO: pass"
        ));
        assert!(!looks_like_import("result = TaskStatus.COMPLETED"));
        assert!(!looks_like_import("logger = logging.getLogger(__name__)"));
    }

    #[test]
    fn looks_like_import_rejects_definition_lines() {
        assert!(!looks_like_import("class TaskStatus(str, Enum):"));
        assert!(!looks_like_import("def get_status(task):"));
    }

    #[test]
    fn detect_investigation_mode_returns_usage_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is TaskStatus used?"),
            InvestigationMode::UsageLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find all references to build_report"),
            InvestigationMode::UsageLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where does TaskStatus appear?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_returns_config_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is the database configured?"),
            InvestigationMode::ConfigLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where logging configuration lives"),
            InvestigationMode::ConfigLookup
        ));
        assert!(matches!(
            detect_investigation_mode("How is the connection configured?"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_returns_initialization_lookup() {
        assert!(matches!(
            detect_investigation_mode("Find where logging is initialized"),
            InvestigationMode::InitializationLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find logging initialization"),
            InvestigationMode::InitializationLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find code that can initialize logging"),
            InvestigationMode::InitializationLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where logging is initialised"),
            InvestigationMode::General
        ));
    }

    #[test]
    fn detect_investigation_mode_returns_definition_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is TaskStatus defined?"),
            InvestigationMode::DefinitionLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where is the TaskRunner declared?"),
            InvestigationMode::DefinitionLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_returns_general() {
        assert!(matches!(
            detect_investigation_mode("What does run_turns do?"),
            InvestigationMode::General
        ));
        assert!(matches!(
            detect_investigation_mode("Explain the TaskRunner"),
            InvestigationMode::General
        ));
    }

    #[test]
    fn detect_investigation_mode_usage_priority_over_config() {
        assert!(matches!(
            detect_investigation_mode("Where is the configured value used?"),
            InvestigationMode::UsageLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where is configuration used?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_usage_priority_over_initialization() {
        assert!(matches!(
            detect_investigation_mode("Where is logging initialization used?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_config_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is config defined?"),
            InvestigationMode::ConfigLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find config for logging"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_config_priority_over_initialization() {
        assert!(matches!(
            detect_investigation_mode("Find where logging configuration is initialized"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_initialization_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is initialization defined?"),
            InvestigationMode::InitializationLookup
        ));
    }

    #[test]
    fn contains_initialization_term_matches_exact_allowed_substrings_only() {
        assert!(contains_initialization_term("def initialize_logging():"));
        assert!(contains_initialization_term(
            "# logging is initialized here"
        ));
        assert!(contains_initialization_term("logging initialization entry"));
        assert!(!contains_initialization_term("setup_logging()"));
        assert!(!contains_initialization_term("bootstrap logging"));
        assert!(!contains_initialization_term("logging is initialised here"));
    }

    #[test]
    fn is_config_file_accepts_standard_extensions() {
        assert!(is_config_file("config/database.yaml"));
        assert!(is_config_file("config/app.yml"));
        assert!(is_config_file("Cargo.toml"));
        assert!(is_config_file("config/settings.json"));
        assert!(is_config_file("config/app.ini"));
        assert!(is_config_file("deploy/app.cfg"));
        assert!(is_config_file("config/logging.conf"));
        assert!(is_config_file("config/db.properties"));
    }

    #[test]
    fn is_config_file_accepts_env_dotfiles() {
        assert!(is_config_file(".env"));
        assert!(is_config_file("config/.env"));
        assert!(!is_config_file(".env.local"));
        assert!(!is_config_file(".env.production"));
    }

    #[test]
    fn is_config_file_rejects_source_files() {
        assert!(!is_config_file("services/task_service.py"));
        assert!(!is_config_file("src/runtime/engine.rs"));
        assert!(!is_config_file("models/enums.py"));
        assert!(!is_config_file("main.go"));
    }

    #[test]
    fn detect_investigation_mode_returns_create_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is the session created?"),
            InvestigationMode::CreateLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where tasks are created"),
            InvestigationMode::CreateLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where does task creation happen?"),
            InvestigationMode::CreateLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_create_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is the session created and defined?"),
            InvestigationMode::CreateLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_initialization_priority_over_create() {
        assert!(matches!(
            detect_investigation_mode("Find where the session is initialized and created"),
            InvestigationMode::InitializationLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_usage_priority_over_create() {
        assert!(matches!(
            detect_investigation_mode("Where is the session used and created?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_config_priority_over_create() {
        assert!(matches!(
            detect_investigation_mode("Where is the session configured and created?"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn contains_create_term_matches_exact_allowed_substrings_only() {
        assert!(contains_create_term("db.create(session)"));
        assert!(contains_create_term("session was created here"));
        assert!(contains_create_term("handles session creation"));
        assert!(contains_create_term("Session.Create()"));
        assert!(contains_create_term("CREATED_AT timestamp"));
        assert!(contains_create_term("recreate the session"));
        assert!(contains_create_term("createTable migration"));
        assert!(!contains_create_term("def handle_session(s):"));
        assert!(!contains_create_term("return session_id"));
    }

    #[test]
    fn detect_investigation_mode_returns_register_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is the command registered?"),
            InvestigationMode::RegisterLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where handlers register commands"),
            InvestigationMode::RegisterLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where does command registration happen?"),
            InvestigationMode::RegisterLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_create_priority_over_register() {
        assert!(matches!(
            detect_investigation_mode("Where is the command created and registered?"),
            InvestigationMode::CreateLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_register_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is the command registered and defined?"),
            InvestigationMode::RegisterLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_usage_priority_over_register() {
        assert!(matches!(
            detect_investigation_mode("Where is the registered command used?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_config_priority_over_register() {
        assert!(matches!(
            detect_investigation_mode("Where is command registration configured?"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_initialization_priority_over_register() {
        assert!(matches!(
            detect_investigation_mode("Find where command registration is initialized"),
            InvestigationMode::InitializationLookup
        ));
    }

    #[test]
    fn contains_register_term_matches_exact_allowed_substrings_only() {
        assert!(contains_register_term("registry.register(command)"));
        assert!(contains_register_term("command was registered here"));
        assert!(contains_register_term("command registration lives here"));
        assert!(contains_register_term("Registry.Register(command)"));
        assert!(contains_register_term("REGISTERED_COMMANDS"));
        assert!(contains_register_term("reregister command handlers"));
        assert!(contains_register_term("registration_notes = []"));
        assert!(!contains_register_term("def handle_command(command):"));
        assert!(!contains_register_term("return command_id"));
    }

    #[test]
    fn detect_investigation_mode_returns_load_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is the session loaded?"),
            InvestigationMode::LoadLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where session loading happens"),
            InvestigationMode::LoadLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where do handlers load sessions?"),
            InvestigationMode::LoadLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_register_priority_over_load() {
        assert!(matches!(
            detect_investigation_mode("Where is the command registered and loaded?"),
            InvestigationMode::RegisterLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_load_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is the session loaded and defined?"),
            InvestigationMode::LoadLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_usage_priority_over_load() {
        assert!(matches!(
            detect_investigation_mode("Where is the loaded session used?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_config_priority_over_load() {
        assert!(matches!(
            detect_investigation_mode("Where is loaded config configured?"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_initialization_priority_over_load() {
        assert!(matches!(
            detect_investigation_mode("Find where session loading is initialized"),
            InvestigationMode::InitializationLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_create_priority_over_load() {
        assert!(matches!(
            detect_investigation_mode("Find where the loaded session is created"),
            InvestigationMode::CreateLookup
        ));
    }

    #[test]
    fn contains_load_term_matches_exact_allowed_substrings_only() {
        assert!(contains_load_term("session = load_session(session_id)"));
        assert!(contains_load_term("session was loaded here"));
        assert!(contains_load_term("session loading happens here"));
        assert!(contains_load_term("Session.Load()"));
        assert!(contains_load_term("LOADED_SESSION"));
        assert!(contains_load_term("session loader"));
        assert!(contains_load_term("reload session"));
        assert!(contains_load_term("autoload session"));
        assert!(!contains_load_term("def handle_session(session):"));
        assert!(!contains_load_term("return session_id"));
    }

    #[test]
    fn detect_investigation_mode_returns_save_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is the session saved?"),
            InvestigationMode::SaveLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where session saving happens"),
            InvestigationMode::SaveLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Where do handlers save sessions?"),
            InvestigationMode::SaveLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_load_priority_over_save() {
        assert!(matches!(
            detect_investigation_mode("Where is the session loaded and saved?"),
            InvestigationMode::LoadLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_save_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is the session saved and defined?"),
            InvestigationMode::SaveLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_usage_priority_over_save() {
        assert!(matches!(
            detect_investigation_mode("Where is the saved session used?"),
            InvestigationMode::UsageLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_config_priority_over_save() {
        assert!(matches!(
            detect_investigation_mode("Where is saved config configured?"),
            InvestigationMode::ConfigLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_initialization_priority_over_save() {
        assert!(matches!(
            detect_investigation_mode("Find where session saving is initialized"),
            InvestigationMode::InitializationLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_create_priority_over_save() {
        assert!(matches!(
            detect_investigation_mode("Find where the saved session is created"),
            InvestigationMode::CreateLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_register_priority_over_save() {
        assert!(matches!(
            detect_investigation_mode("Find where the saved command is registered"),
            InvestigationMode::RegisterLookup
        ));
    }

    #[test]
    fn contains_save_term_matches_exact_allowed_substrings_only() {
        assert!(contains_save_term("save_session(session)"));
        assert!(contains_save_term("session was saved here"));
        assert!(contains_save_term("session saving happens here"));
        assert!(contains_save_term("Session.Save()"));
        assert!(contains_save_term("SAVED_SESSION"));
        assert!(contains_save_term("autosave session"));
        assert!(contains_save_term("savepoint created"));
        assert!(contains_save_term("saved_at timestamp"));
        assert!(!contains_save_term("def handle_session(session):"));
        assert!(!contains_save_term("return session_id"));
    }

    // candidate_preference_hint tests

    fn make_search_output_for_hint(matches: Vec<(&str, &str)>) -> crate::tools::ToolOutput {
        use crate::tools::types::{SearchMatch, SearchResultsOutput};
        let matches: Vec<SearchMatch> = matches
            .into_iter()
            .enumerate()
            .map(|(i, (file, line))| SearchMatch {
                file: file.to_string(),
                line_number: i + 1,
                line: line.to_string(),
            })
            .collect();
        let total = matches.len();
        crate::tools::ToolOutput::SearchResults(SearchResultsOutput {
            query: "test".into(),
            matches,
            total_matches: total,
            truncated: false,
        })
    }

    #[test]
    fn candidate_preference_hint_returns_none_when_no_candidates() {
        let state = InvestigationState::new();
        assert!(state
            .candidate_preference_hint(InvestigationMode::InitializationLookup)
            .is_none());
    }

    #[test]
    fn candidate_preference_hint_initialization_fires_with_mixed_candidates() {
        let mut state = InvestigationState::new();
        // z_init.py has an initialization term; commands.py does not
        let output = make_search_output_for_hint(vec![
            ("sandbox/cli/commands.py", "import logging"),
            ("sandbox/init/z_init.py", "def initialize_logging(): pass"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::InitializationLookup);
        assert!(
            hint.is_some(),
            "hint must fire when init candidate exists alongside non-init"
        );
        assert!(
            hint.unwrap().contains("sandbox/init/z_init.py"),
            "hint must name the initialization candidate"
        );
    }

    #[test]
    fn candidate_preference_hint_initialization_suppressed_when_all_init() {
        let mut state = InvestigationState::new();
        // Both files have initialization terms — no non-init candidates exist
        let output = make_search_output_for_hint(vec![
            ("sandbox/init/a.py", "logging.initialize()"),
            ("sandbox/init/b.py", "def initialization_setup(): pass"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::InitializationLookup);
        assert!(
            hint.is_none(),
            "hint must not fire when all candidates are initialization files"
        );
    }

    #[test]
    fn candidate_preference_hint_config_fires_with_mixed_candidates() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            (
                "services/database.py",
                "DATABASE_URL = os.getenv(\"DATABASE_URL\")",
            ),
            (
                "config/database.yaml",
                "database:\n  url: postgres://localhost/mydb",
            ),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::ConfigLookup);
        assert!(
            hint.is_some(),
            "hint must fire when config candidate exists alongside source"
        );
        assert!(
            hint.unwrap().contains("config/database.yaml"),
            "hint must name the config file candidate"
        );
    }

    #[test]
    fn candidate_preference_hint_config_suppressed_when_no_config_candidates() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            (
                "services/database.py",
                "DATABASE_URL = os.getenv(\"DATABASE_URL\")",
            ),
            ("services/user.py", "USER = UserService()"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::ConfigLookup);
        assert!(
            hint.is_none(),
            "hint must not fire when no config-file candidates exist"
        );
    }

    #[test]
    fn candidate_preference_hint_general_mode_returns_none() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("sandbox/init/z_init.py", "logging.basicConfig()"),
            ("sandbox/cli/commands.py", "import logging"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        assert!(
            state
                .candidate_preference_hint(InvestigationMode::General)
                .is_none(),
            "General mode must produce no candidate hint"
        );
    }

    #[test]
    fn candidate_preference_hint_definition_lookup_returns_none() {
        // DefinitionLookup is handled by definition_site_file in rendering — no hint here
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("models/enums.py", "class TaskStatus(str, Enum):"),
            ("cli/commands.py", "from models.enums import TaskStatus"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        assert!(
            state
                .candidate_preference_hint(InvestigationMode::DefinitionLookup)
                .is_none(),
            "DefinitionLookup must not produce a candidate hint — handled by definition_site_file"
        );
    }

    #[test]
    fn candidate_preference_hint_names_first_init_candidate_in_search_order() {
        let mut state = InvestigationState::new();
        // Non-init first, then two init candidates — hint must name the first init candidate
        let output = make_search_output_for_hint(vec![
            ("sandbox/cli/commands.py", "import logging"),
            ("sandbox/init/a.py", "logging.initialize()"),
            ("sandbox/init/b.py", "def initialization_setup(): pass"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::InitializationLookup);
        assert!(hint.is_some());
        let hint = hint.unwrap();
        assert!(
            hint.contains("sandbox/init/a.py"),
            "hint must name the first init candidate in search order, got: {hint}"
        );
        assert!(
            !hint.contains("sandbox/init/b.py"),
            "hint must not name second candidate when first already named"
        );
    }

    #[test]
    fn candidate_preference_hint_is_deterministic_for_same_inputs() {
        let mut state1 = InvestigationState::new();
        let mut state2 = InvestigationState::new();
        let matches = vec![
            ("sandbox/cli/commands.py", "import logging"),
            ("sandbox/init/z_init.py", "def initialize_logging(): pass"),
        ];
        let output1 = make_search_output_for_hint(matches.clone());
        let output2 = make_search_output_for_hint(matches);
        state1.record_search_results(&output1, None, &mut |_| {});
        state2.record_search_results(&output2, None, &mut |_| {});
        assert_eq!(
            state1.candidate_preference_hint(InvestigationMode::InitializationLookup),
            state2.candidate_preference_hint(InvestigationMode::InitializationLookup),
            "candidate_preference_hint must be deterministic for identical inputs"
        );
    }

    #[test]
    fn candidate_preference_hint_usage_lookup_returns_none() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("sandbox/init/z_init.py", "logging.basicConfig()"),
            ("sandbox/cli/commands.py", "logger.info(\"hello\")"),
        ]);
        state.record_search_results(&output, None, &mut |_| {});
        assert!(
            state
                .candidate_preference_hint(InvestigationMode::UsageLookup)
                .is_none(),
            "UsageLookup must produce no candidate hint"
        );
    }

    #[test]
    fn preferred_usage_candidate_prefers_substantive_source_over_import_only_and_definition() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("models/enums.py", "class TaskStatus(str, Enum):"),
            ("cli/header.py", "from models.enums import TaskStatus"),
            (
                "services/runner.py",
                "if task.status == TaskStatus.PENDING:",
            ),
            ("services/runner.py", "audit_status(TaskStatus.PENDING)"),
        ]);
        state.record_search_results(&output, Some("TaskStatus"), &mut |_| {});

        assert_eq!(
            state.preferred_usage_candidate().as_deref(),
            Some("services/runner.py"),
            "substantive source file should outrank definition-only and import-only candidates"
        );
    }

    #[test]
    fn preferred_usage_candidate_prefers_normal_source_over_initialization_candidate() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("models/enums.py", "class TaskStatus(str, Enum):"),
            (
                "sandbox/init/bootstrap.py",
                "initialize_task_status(TaskStatus.PENDING)",
            ),
            (
                "sandbox/init/bootstrap.py",
                "INITIALIZED_STATUS = TaskStatus.PENDING",
            ),
            (
                "sandbox/services/runner.py",
                "if task.status == TaskStatus.PENDING:",
            ),
        ]);
        state.record_search_results(&output, Some("TaskStatus"), &mut |_| {});

        assert_eq!(
            state.preferred_usage_candidate().as_deref(),
            Some("sandbox/services/runner.py"),
            "normal source files should outrank initialization candidates for UsageLookup"
        );
    }

    #[test]
    fn best_candidate_for_mode_general_prefers_source_over_docs_and_benchmarks() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("sandbox/README.md", "Completed tasks are documented here."),
            (
                "docs/benchmarks/runs/2026-04-29-phase16-baseline.md",
                "completed tasks benchmark notes",
            ),
            (
                "sandbox/services/task_service.py",
                "if task.completed:\n    filtered.append(task)",
            ),
        ]);
        state.record_search_results(&output, Some("completed"), &mut |_| {});

        assert_eq!(
            state.best_candidate_for_mode(InvestigationMode::General),
            Some("sandbox/services/task_service.py"),
            "General candidate preference should pick source over README/docs/benchmarks"
        );
    }

    #[test]
    fn preferred_usage_candidate_is_deterministic_for_same_inputs() {
        let matches = vec![
            ("models/enums.py", "class TaskStatus(str, Enum):"),
            ("cli/header.py", "from models.enums import TaskStatus"),
            (
                "services/runner.py",
                "if task.status == TaskStatus.PENDING:",
            ),
        ];
        let mut state1 = InvestigationState::new();
        let mut state2 = InvestigationState::new();
        let output1 = make_search_output_for_hint(matches.clone());
        let output2 = make_search_output_for_hint(matches);
        state1.record_search_results(&output1, Some("TaskStatus"), &mut |_| {});
        state2.record_search_results(&output2, Some("TaskStatus"), &mut |_| {});

        assert_eq!(
            state1.preferred_usage_candidate(),
            state2.preferred_usage_candidate(),
            "preferred usage candidate selection must be deterministic"
        );
    }

    #[test]
    fn definition_of_symbol_rejects_superstring_identifier() {
        assert!(!looks_like_definition_of_symbol(
            "class TaskStatus:",
            "Task"
        ));
        assert!(!looks_like_definition_of_symbol(
            "class TaskStatusEnum:",
            "Task"
        ));
        assert!(!looks_like_definition_of_symbol(
            "pub struct TaskRunner {",
            "Task"
        ));
        assert!(!looks_like_definition_of_symbol("fn create_task()", "task"));
    }

    #[test]
    fn definition_of_symbol_accepts_exact_identifier() {
        assert!(looks_like_definition_of_symbol("class Task:", "Task"));
        assert!(looks_like_definition_of_symbol("class Task(Base):", "Task"));
        assert!(looks_like_definition_of_symbol(
            "class Task(str, Enum):",
            "Task"
        ));
    }

    #[test]
    fn definition_of_symbol_accepts_exact_symbol_across_languages() {
        assert!(looks_like_definition_of_symbol(
            "class TaskStatus(str, Enum):",
            "TaskStatus"
        ));
        assert!(looks_like_definition_of_symbol(
            "pub struct TaskStatus {",
            "TaskStatus"
        ));
        assert!(looks_like_definition_of_symbol(
            "pub enum TaskStatus {",
            "TaskStatus"
        ));
        assert!(looks_like_definition_of_symbol(
            "def TaskStatus(self):",
            "TaskStatus"
        ));
        assert!(looks_like_definition_of_symbol(
            "func TaskStatus() error {",
            "TaskStatus"
        ));
        assert!(looks_like_definition_of_symbol(
            "function TaskStatus() {",
            "TaskStatus"
        ));
        assert!(looks_like_definition_of_symbol(
            "interface TaskStatus {",
            "TaskStatus"
        ));
    }

    #[test]
    fn definition_only_classification_uses_exact_symbol_when_query_given() {
        // query="Task": "class TaskStatus:" must NOT be definition-only —
        // the file has a non-definition match for the symbol Task.
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![(
            "models/task_status.py",
            "class TaskStatus(str, Enum):",
        )]);
        state.record_search_results(&output, Some("Task"), &mut |_| {});
        assert!(
            !state
                .definition_only_candidates
                .contains("models/task_status.py"),
            "class TaskStatus must not be definition-only for symbol 'Task'"
        );
        assert!(
            state.has_non_definition_candidates,
            "has_non_definition_candidates must be set when no exact-symbol definition exists"
        );
    }

    #[test]
    fn definition_only_classification_accepts_exact_symbol_match() {
        // query="Task": "class Task:" IS a definition-only line.
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![("models/task.py", "class Task(Base):")]);
        state.record_search_results(&output, Some("Task"), &mut |_| {});
        assert!(
            state.definition_only_candidates.contains("models/task.py"),
            "class Task must be definition-only for symbol 'Task'"
        );
        assert!(
            !state.has_non_definition_candidates,
            "has_non_definition_candidates must not be set when only exact definition exists"
        );
    }

    #[test]
    fn definition_only_classification_taskstatus_still_works() {
        // Regression: query="TaskStatus" — "class TaskStatus:" must still be definition-only.
        let mut state = InvestigationState::new();
        let output =
            make_search_output_for_hint(vec![("models/enums.py", "class TaskStatus(str, Enum):")]);
        state.record_search_results(&output, Some("TaskStatus"), &mut |_| {});
        assert!(
            state.definition_only_candidates.contains("models/enums.py"),
            "class TaskStatus must be definition-only for symbol 'TaskStatus'"
        );
    }

    fn make_file_contents_output(path: &str, contents: &str) -> crate::tools::ToolOutput {
        use crate::tools::types::FileContentsOutput;
        crate::tools::ToolOutput::FileContents(FileContentsOutput {
            path: path.to_string(),
            contents: contents.to_string(),
            total_lines: contents.lines().count(),
            truncated: false,
        })
    }

    #[test]
    fn direct_read_does_not_increment_candidate_counts() {
        let mut state = InvestigationState::new();
        let output = make_file_contents_output("src/foo.rs", "fn main() {}");
        state.record_read_result(&output, InvestigationMode::General, ReadClassification::Direct, &mut |_| {});
        assert_eq!(state.direct_reads_count, 1);
        assert!(state.direct_read_paths.contains("src/foo.rs"));
        assert_eq!(state.candidate_reads_count, 0);
        assert_eq!(state.useful_accepted_candidate_reads, 0);
    }

    #[test]
    fn direct_read_returns_no_recovery() {
        let mut state = InvestigationState::new();
        let output = make_file_contents_output("src/foo.rs", "fn main() {}");
        let result = state.record_read_result(&output, InvestigationMode::General, ReadClassification::Direct, &mut |_| {});
        assert!(result.is_none());
    }

    #[test]
    fn candidate_read_path_unchanged() {
        let mut state = InvestigationState::new();
        let search_output = make_search_output_for_hint(vec![("src/foo.rs", "fn main()")]);
        state.record_search_results(&search_output, None, &mut |_| {});
        let output = make_file_contents_output("src/foo.rs", "fn main() {}");
        state.record_read_result(&output, InvestigationMode::General, ReadClassification::Candidate, &mut |_| {});
        assert_eq!(state.candidate_reads_count, 1);
        assert_eq!(state.direct_reads_count, 0);
        assert!(state.direct_read_paths.is_empty());
    }

    // CallSiteLookup tests

    #[test]
    fn detect_investigation_mode_returns_call_site_lookup() {
        assert!(matches!(
            detect_investigation_mode("Where is process_task called?"),
            InvestigationMode::CallSiteLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find where process_task is invoked"),
            InvestigationMode::CallSiteLookup
        ));
        assert!(matches!(
            detect_investigation_mode("What calls run_turn?"),
            InvestigationMode::CallSiteLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Show the invocation of dispatch"),
            InvestigationMode::CallSiteLookup
        ));
        assert!(matches!(
            detect_investigation_mode("What is used by the scheduler?"),
            InvestigationMode::CallSiteLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_call_site_priority_over_usage() {
        assert!(matches!(
            detect_investigation_mode("Where is run_task called and used?"),
            InvestigationMode::CallSiteLookup
        ));
        assert!(matches!(
            detect_investigation_mode("Find functions that invoke and reference process_task"),
            InvestigationMode::CallSiteLookup
        ));
    }

    #[test]
    fn detect_investigation_mode_call_site_priority_over_definition() {
        assert!(matches!(
            detect_investigation_mode("Where is dispatch called and defined?"),
            InvestigationMode::CallSiteLookup
        ));
    }

    #[test]
    fn looks_like_call_expression_of_symbol_accepts_direct_call() {
        assert!(looks_like_call_expression_of_symbol(
            "    process_task(my_task)",
            "process_task"
        ));
        assert!(looks_like_call_expression_of_symbol(
            "let result = process_task(args);",
            "process_task"
        ));
        assert!(looks_like_call_expression_of_symbol(
            "self.process_task(args)",
            "process_task"
        ));
    }

    #[test]
    fn looks_like_call_expression_of_symbol_rejects_definition() {
        assert!(!looks_like_call_expression_of_symbol(
            "pub fn process_task(t: Task) {",
            "process_task"
        ));
        assert!(!looks_like_call_expression_of_symbol(
            "fn process_task(t: Task) -> Result<()> {",
            "process_task"
        ));
        assert!(!looks_like_call_expression_of_symbol(
            "def process_task(self, task):",
            "process_task"
        ));
    }

    #[test]
    fn looks_like_call_expression_of_symbol_rejects_non_call_reference() {
        // Reference without parentheses — not a call expression
        assert!(!looks_like_call_expression_of_symbol(
            "let f = process_task;",
            "process_task"
        ));
        assert!(!looks_like_call_expression_of_symbol(
            "// calls process_task somewhere",
            "process_task"
        ));
    }

    #[test]
    fn call_site_gate_dispatches_to_call_site_candidate() {
        let mut state = InvestigationState::new();
        let search_output = make_search_output_for_hint(vec![
            ("src/definitions.rs", "pub fn process_task(t: Task) {"),
            ("src/callers.rs", "process_task(my_task)"),
        ]);
        state.record_search_results(&search_output, Some("process_task"), &mut |_| {});

        assert!(
            state.call_site_candidates.contains("src/callers.rs"),
            "callers.rs must be classified as a call-site candidate"
        );
        assert!(
            !state.call_site_candidates.contains("src/definitions.rs"),
            "definitions.rs must not be classified as a call-site candidate"
        );

        let read_output =
            make_file_contents_output("src/definitions.rs", "pub fn process_task(t: Task) {}");
        let recovery = state.record_read_result(
            &read_output,
            InvestigationMode::CallSiteLookup,
            ReadClassification::Candidate,
            &mut |_| {},
        );
        assert!(
            recovery.is_some(),
            "gate must fire a recovery for a non-call-site read"
        );
        let (path, _) = recovery.unwrap();
        assert_eq!(
            path, "src/callers.rs",
            "recovery must redirect to the call-site candidate"
        );
    }

    #[test]
    fn call_site_gate_accepts_when_no_call_site_candidates() {
        let mut state = InvestigationState::new();
        let search_output = make_search_output_for_hint(vec![(
            "src/definitions.rs",
            "pub fn process_task(t: Task) {",
        )]);
        state.record_search_results(&search_output, Some("process_task"), &mut |_| {});

        assert!(
            state.call_site_candidates.is_empty(),
            "call_site_candidates must be empty when no call-expression lines exist"
        );

        let read_output =
            make_file_contents_output("src/definitions.rs", "pub fn process_task(t: Task) {}");
        let recovery = state.record_read_result(
            &read_output,
            InvestigationMode::CallSiteLookup,
            ReadClassification::Candidate,
            &mut |_| {},
        );
        assert!(
            recovery.is_none(),
            "gate must not fire when no call-site candidates exist"
        );
        assert_eq!(
            state.useful_accepted_candidate_reads, 1,
            "read must be accepted as useful evidence when no call-site candidates exist"
        );
    }

    #[test]
    fn candidate_preference_hint_call_site_fires_with_mixed_candidates() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("src/definitions.rs", "pub fn process_task(t: Task) {"),
            ("src/callers.rs", "process_task(my_task)"),
        ]);
        state.record_search_results(&output, Some("process_task"), &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::CallSiteLookup);
        assert!(
            hint.is_some(),
            "hint must fire when call-site candidate exists alongside non-call-site"
        );
        assert!(
            hint.unwrap().contains("src/callers.rs"),
            "hint must name the call-site candidate"
        );
    }

    #[test]
    fn candidate_preference_hint_call_site_suppressed_when_all_call_sites() {
        let mut state = InvestigationState::new();
        let output = make_search_output_for_hint(vec![
            ("src/a.rs", "process_task(task_a)"),
            ("src/b.rs", "process_task(task_b)"),
        ]);
        state.record_search_results(&output, Some("process_task"), &mut |_| {});
        let hint = state.candidate_preference_hint(InvestigationMode::CallSiteLookup);
        assert!(
            hint.is_none(),
            "hint must not fire when all candidates are call-site files"
        );
    }
}
