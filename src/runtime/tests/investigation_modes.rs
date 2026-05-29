use super::*;

#[test]
fn config_lookup_non_config_read_dispatches_to_config_file() {
    // Config lookup: two candidates — a source file and a config file.
    // Model reads the source file first → runtime dispatches directly to config.yaml.
    // No text correction is injected. The dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("config")).unwrap();
    fs::write(
        tmp.path().join("services").join("database.py"),
        "DATABASE_URL = os.getenv(\"DATABASE_URL\")\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("config").join("database.yaml"),
        "database:\n  url: postgres://localhost/mydb\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: database]",
            "[read_file: services/database.py]",
            "The database is configured in config/database.yaml.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is the database configured?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to config candidate must admit synthesis: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("The database is configured in config/database.yaml.")
    );
}

#[test]
fn config_lookup_no_config_candidates_degrades_cleanly() {
    // Config lookup triggered, but no config-file candidates exist (source files only).
    // has_non_config_candidates = true, config_file_candidates is empty.
    // Gate 2 does not fire — source file read is accepted → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("database.py"),
        "DATABASE_URL = os.getenv(\"DATABASE_URL\")\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: database]",
            "[read_file: services/database.py]",
            "The database connection is set up in services/database.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is the database configured?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "config lookup with no config candidates must degrade to acceptance: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("The database connection is set up in services/database.py.")
    );
}

#[test]
fn create_lookup_non_create_read_dispatches_to_create_file() {
    // File A: no create-term matches → non-create candidate.
    // File B: a create-term match → create candidate.
    // Model reads A first → runtime dispatches directly to B. No text correction injected.
    // Dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("storage")).unwrap();
    fs::write(
        tmp.path().join("services").join("task_handler.py"),
        "def handle_task(task):\n    task.run()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("storage").join("task_store.py"),
        "def store_task(task):\n    db.create(task)\n    return task.id\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: task]",
            "[read_file: services/task_handler.py]",
            "Tasks are created in storage/task_store.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are tasks created?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to create candidate must admit synthesis: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Tasks are created in storage/task_store.py.")
    );
}

#[test]
fn create_lookup_no_create_candidates_degrades_cleanly() {
    // All candidates have no create-term matches.
    // Gate does not fire — any read is accepted (fallback behavior).
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("task_handler.py"),
        "def handle_task(task):\n    task.run()\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: task]",
            "[read_file: services/task_handler.py]",
            "Tasks are handled in services/task_handler.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are tasks created?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "create lookup with no create candidates must degrade to acceptance: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Tasks are handled in services/task_handler.py.")
    );
}

#[test]
fn create_lookup_non_create_read_dispatch_then_ignored_tool_call_succeeds() {
    // Model reads non-create file → runtime dispatches to create candidate (task_store.py).
    // Dispatched read makes evidence ready. Model then tries another tool call (rejected by
    // answer_phase guard) and on the follow-up produces the answer → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("handlers")).unwrap();
    fs::create_dir_all(tmp.path().join("storage")).unwrap();
    fs::write(
        tmp.path().join("services").join("runner.py"),
        "def run_task(task):\n    task.execute()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("handlers").join("task_handler.py"),
        "def handle_task(task):\n    pass\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("storage").join("task_store.py"),
        "def store_task(task):\n    db.create(task)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: task]",
            "[read_file: services/runner.py]",
            "[read_file: handlers/task_handler.py]",
            "Tasks run in services/runner.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are tasks created?".into(),
        },
    );

    assert!(!has_failed(&events), "must complete cleanly: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to create candidate must complete as ToolAssisted: {answer_source:?}"
    );
}

#[test]
fn create_lookup_noisy_create_term_in_comment_still_classifies_as_create() {
    // A line like "# TODO: create session handling" contains "create" as substring.
    // The classification is structural/substring — comments match the same as code.
    // This tests the known noisy behavior described in the spec.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("models")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_service.py"),
        "# TODO: create session handling\ndef get_session(sid):\n    return db.get(sid)\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("models").join("session.py"),
        "class Session:\n    pass\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_service.py]",
            "Sessions are handled in services/session_service.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions created?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    assert!(
        !snapshot
            .iter()
            .any(|m| m.content.contains("creation lookup")),
        "no recovery expected when create candidate is read first"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "create candidate read must admit synthesis: {answer_source:?}"
    );
}

#[test]
fn register_lookup_non_register_read_dispatches_to_register_file() {
    // File A: no register-term matches → non-register candidate.
    // File B: a register-term match → register candidate.
    // Model reads A first → runtime dispatches directly to B. No text correction injected.
    // Dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("cli")).unwrap();
    fs::write(
        tmp.path().join("cli").join("handlers.py"),
        "def handle_command(command):\n    return command.run()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("cli").join("registry.py"),
        "def wire_command(command):\n    registry.register(command)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: command]",
            "[read_file: cli/handlers.py]",
            "Commands are registered in cli/registry.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are commands registered?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to register candidate must admit synthesis: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Commands are registered in cli/registry.py.")
    );
}

#[test]
fn register_lookup_no_register_candidates_degrades_cleanly() {
    // All candidates have no register-term matches.
    // Gate does not fire — any read is accepted (fallback behavior).
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("cli")).unwrap();
    fs::write(
        tmp.path().join("cli").join("handlers.py"),
        "def handle_command(command):\n    return command.run()\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: command]",
            "[read_file: cli/handlers.py]",
            "Commands are handled in cli/handlers.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are commands registered?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "register lookup with no register candidates must degrade to acceptance: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Commands are handled in cli/handlers.py.")
    );
}

#[test]
fn register_lookup_non_register_read_dispatch_then_ignored_tool_call_succeeds() {
    // Model reads non-register file → runtime dispatches to register candidate (registry.py).
    // Dispatched read makes evidence ready. Model then tries another tool call (rejected by
    // answer_phase guard) and on the follow-up produces the answer → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("cli")).unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("cli").join("handlers.py"),
        "def handle_command(command):\n    return command.run()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("command_runner.py"),
        "def run_command(command):\n    command.run()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("cli").join("registry.py"),
        "def wire_command(command):\n    registry.register(command)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: command]",
            "[read_file: cli/handlers.py]",
            "[read_file: services/command_runner.py]",
            "Commands are registered in cli/handlers.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are commands registered?".into(),
        },
    );

    assert!(!has_failed(&events), "must complete cleanly: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to register candidate must complete as ToolAssisted: {answer_source:?}"
    );
}

#[test]
fn register_lookup_noisy_register_term_in_comment_still_classifies_as_register() {
    // A line like "# TODO: register command handler" contains "register".
    // The classification is structural/substring — comments match the same as code.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("cli")).unwrap();
    fs::write(
        tmp.path().join("cli").join("commands.py"),
        "# TODO: register command handler\ndef command_handler(command):\n    return command.run()\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: command]",
            "[read_file: cli/commands.py]",
            "Commands are handled in cli/commands.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are commands registered?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    assert!(
        !snapshot
            .iter()
            .any(|m| m.content.contains("registration lookup")),
        "no recovery expected when register candidate is read first"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "register candidate read must admit synthesis: {answer_source:?}"
    );
}

#[test]
fn load_lookup_non_load_read_dispatches_to_load_file() {
    // File A: no load-term matches → non-load candidate.
    // File B: a load-term match → load candidate.
    // Model reads A first → runtime dispatches directly to B. No text correction injected.
    // Dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("session_loader.py"),
        "def get_session(session_id):\n    return load_session(session_id)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_handler.py]",
            "Sessions are loaded in services/session_loader.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to load candidate must admit synthesis: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are loaded in services/session_loader.py.")
    );
}

#[test]
fn load_lookup_no_load_candidates_degrades_cleanly() {
    // All candidates have no load-term matches.
    // Gate does not fire — any read is accepted (fallback behavior).
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_handler.py]",
            "Sessions are handled in services/session_handler.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "load lookup with no load candidates must degrade to acceptance: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are handled in services/session_handler.py.")
    );
}

#[test]
fn load_lookup_non_load_read_dispatch_then_ignored_tool_call_succeeds() {
    // Model reads non-load file → runtime dispatches to load candidate (session_loader.py).
    // Dispatched read makes evidence ready. Model then tries another tool call (rejected by
    // answer_phase guard) and on the follow-up produces the answer → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("controllers")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("controllers").join("session_controller.py"),
        "def show_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("session_loader.py"),
        "def get_session(session_id):\n    return load_session(session_id)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_handler.py]",
            "[read_file: controllers/session_controller.py]",
            "Sessions are loaded in services/session_handler.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    assert!(!has_failed(&events), "must complete cleanly: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to load candidate must complete as ToolAssisted: {answer_source:?}"
    );
}

#[test]
fn load_lookup_noisy_load_term_in_comment_still_classifies_as_load() {
    // A line like "# TODO: load session data" contains "load".
    // The classification is structural/substring — comments match the same as code.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_service.py"),
        "# TODO: load session data\ndef handle_session(session):\n    return session.id\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_service.py]",
            "Sessions are handled in services/session_service.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    assert!(
        !snapshot.iter().any(|m| m.content.contains("load lookup")),
        "no recovery expected when load candidate is read first"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "load candidate read must admit synthesis: {answer_source:?}"
    );
}

#[test]
fn save_lookup_non_save_read_dispatches_to_save_file() {
    // File A: no save-term matches → non-save candidate.
    // File B: a save-term match → save candidate.
    // Model reads A first → runtime dispatches directly to B. No text correction injected.
    // Dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("session_store.py"),
        "def store_session(session):\n    save_session(session)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_handler.py]",
            "Sessions are saved in services/session_store.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions saved?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to save candidate must admit synthesis: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are saved in services/session_store.py.")
    );
}

#[test]
fn save_lookup_no_save_candidates_degrades_cleanly() {
    // All candidates have no save-term matches.
    // Gate does not fire — any read is accepted (fallback behavior).
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_handler.py]",
            "Sessions are handled in services/session_handler.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions saved?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "save lookup with no save candidates must degrade to acceptance: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are handled in services/session_handler.py.")
    );
}

#[test]
fn save_lookup_non_save_read_dispatch_then_ignored_tool_call_succeeds() {
    // Model reads non-save file → runtime dispatches to save candidate (session_store.py).
    // Dispatched read makes evidence ready. Model then tries another tool call (rejected by
    // answer_phase guard) and on the follow-up produces the answer → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::create_dir_all(tmp.path().join("controllers")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_handler.py"),
        "def handle_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("controllers").join("session_controller.py"),
        "def show_session(session):\n    return session.id\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("session_store.py"),
        "def store_session(session):\n    save_session(session)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_handler.py]",
            "[read_file: controllers/session_controller.py]",
            "Sessions are saved in services/session_handler.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions saved?".into(),
        },
    );

    assert!(!has_failed(&events), "must complete cleanly: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to save candidate must complete as ToolAssisted: {answer_source:?}"
    );
}

#[test]
fn save_lookup_noisy_save_term_in_comment_still_classifies_as_save() {
    // A line like "# TODO: save session data" contains "save".
    // The classification is structural/substring — comments match the same as code.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_service.py"),
        "# TODO: save session data\ndef handle_session(session):\n    return session.id\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: session]",
            "[read_file: services/session_service.py]",
            "Sessions are handled in services/session_service.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions saved?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let snapshot = rt.messages_snapshot();
    assert!(
        !snapshot.iter().any(|m| m.content.contains("save lookup")),
        "no recovery expected when save candidate is read first"
    );
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "save candidate read must admit synthesis: {answer_source:?}"
    );
}

#[test]
fn initialization_lookup_wrong_candidate_dispatches_to_init_candidate() {
    // Regression: InitializationLookup — two search candidates, one with init terms,
    // one without. Model reads the non-init candidate first.
    //
    // Old behavior: runtime injected a text correction; model ignored it and re-searched;
    // search budget exhausted → RepeatedSearchBudgetViolation terminal.
    //
    // New behavior: runtime dispatches directly to the init candidate. The dispatched read
    // satisfies evidence. No correction text is injected, no search is reopened → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("app_handler.py"),
        "def handle_request(req):\n    return req.process()\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("app_boot.py"),
        "def initialize_app():\n    app.start()\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: app]",
            "[read_file: services/app_handler.py]",
            "The app is initialized in services/app_boot.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is the app initialized?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");

    let snapshot = rt.messages_snapshot();
    assert!(
        !snapshot
            .iter()
            .any(|m| m.content.contains("initialization lookup")),
        "no text correction must be injected — dispatch replaces it: {snapshot:?}"
    );
    assert!(
        !snapshot
            .iter()
            .any(|m| m.content.contains("search budget exceeded")),
        "search must not be reopened after dispatch: {snapshot:?}"
    );

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch to init candidate must complete as ToolAssisted: {answer_source:?}"
    );
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("The app is initialized in services/app_boot.py.")
    );
}

#[test]
fn load_lookup_definition_only_read_dispatches_to_call_site_candidate() {
    // File A (session_loader.py): load term only on a definition line — load_definition_only candidate.
    // File B (session_service.py): load term on a call-site line — non-definition load candidate.
    // Model searches for "load_session" then reads A first.
    // Gate 6a fires: A is a load candidate but all its load-term lines are definitions.
    // Runtime dispatches directly to B. Dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_loader.py"),
        "def load_session(session_id):\n    return None\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("session_service.py"),
        "result = load_session(user_id)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: load_session]",
            "[read_file: services/session_loader.py]",
            "Sessions are loaded in services/session_service.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "dispatch from load-definition-only to call-site candidate must complete as ToolAssisted: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are loaded in services/session_service.py.")
    );
}

#[test]
fn load_lookup_no_call_site_candidate_produces_insufficient_evidence() {
    // Only candidate has load terms exclusively on definition lines.
    // has_non_definition_load_candidates = false — Gate 6a never fires (no call-site to dispatch to).
    // Model answers without reading → runtime seeds read directly → evidence accepted → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_loader.py"),
        "def load_session(session_id):\n    return None\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: load_session]",
            "load_session is defined in services/session_loader.py.",
            "load_session is defined in services/session_loader.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions loaded?".into(),
        },
    );

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "LoadLookup with no call-site candidate must seed read and produce ToolAssisted: {answer_source:?}"
    );
}

#[test]
fn general_mode_load_definition_only_read_dispatches_to_call_site_candidate() {
    // General mode (query has no load/save/config/etc terms; "handled" triggers investigation
    // without triggering any specific lookup mode).
    // File A (session_loader.py): search match on a definition line containing "load" → load_definition_only candidate.
    // File B (session_service.py): search match on a call-site line containing "load" → non-definition load candidate.
    // Model searches for "load_session" then reads A first.
    // Gate 6a fires in General mode: A is a load_definition_only candidate and a non-definition load candidate exists.
    // Runtime dispatches directly to B. Dispatched read satisfies evidence → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_loader.py"),
        "def load_session(session_id):\n    return None\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("services").join("session_service.py"),
        "result = load_session(user_id)\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: load_session]",
            "[read_file: services/session_loader.py]",
            "Sessions are handled in services/session_service.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions handled?".into(),
        },
    );

    assert!(!has_failed(&events), "turn must not fail: {events:?}");
    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "General mode dispatch from load-definition-only to call-site candidate must complete as ToolAssisted: {answer_source:?}"
    );
    let snapshot = rt.messages_snapshot();
    let last_assistant = snapshot
        .iter()
        .rev()
        .find(|m| m.role == crate::llm::backend::Role::Assistant)
        .map(|m| m.content.as_str());
    assert_eq!(
        last_assistant,
        Some("Sessions are handled in services/session_service.py.")
    );
}

#[test]
fn initialization_lookup_single_candidate_already_read_does_not_loop_to_tool_limit() {
    // Regression: Phase 30.4 introduced a recovery loop when the only initialization
    // candidate was already read and accepted (useful_reads=1) but useful_candidate_reads_target=2
    // (high total_matches raised the target). The premature synthesis correction path dispatched
    // the same file repeatedly via pending_runtime_call; DEDUP blocked each attempt silently,
    // causing the loop to run until ToolLimitReached.
    //
    // The fix: check the return value of issue_premature_synthesis_correction() in the dispatch
    // path and fall through to the terminal when the correction was already issued.
    use crate::runtime::types::RuntimeTerminalReason;
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();

    // Six source files each containing "logging" — search_candidate_paths.len() >= 6
    // raises useful_candidate_reads_target to 2, even though only one file is an init site.
    // (MAX_LINES_COLLECTED_PER_FILE=3 caps rg output per file, so total_matches alone
    // would require 4+ files; using the candidate-count path is simpler and more reliable.)
    for name in &[
        "handler_a.py",
        "handler_b.py",
        "handler_c.py",
        "handler_d.py",
        "handler_e.py",
    ] {
        fs::write(tmp.path().join(name), "import logging\n").unwrap();
    }
    // logging_init.py: the only initialization candidate.
    fs::write(
        tmp.path().join("logging_init.py"),
        "def initialize_logging():\n    pass\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: logging]",
            "[read_file: logging_init.py]",
            // Model answers after reading the only init candidate; evidence target is 2
            // so evidence_ready() is still false. Premature synthesis correction fires once,
            // dispatches the same file, DEDUP blocks it, model answers again.
            "Logging is initialized in logging_init.py.",
            // Second direct answer after the blocked recovery dispatch.
            // With the fix: correction already issued → terminal (InsufficientEvidence).
            // Without the fix: correction re-dispatches the same file in an infinite loop.
            "Logging is initialized in logging_init.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where is logging initialized?".into(),
        },
    );

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        !matches!(answer_source, Some(AnswerSource::ToolLimitReached)),
        "single-candidate recovery must not loop to ToolLimitReached: {answer_source:?}"
    );
    assert!(
        matches!(
            answer_source,
            Some(AnswerSource::RuntimeTerminal {
                reason: RuntimeTerminalReason::InsufficientEvidence,
                ..
            })
        ),
        "must terminate with InsufficientEvidence after one blocked recovery: {answer_source:?}"
    );
}

#[test]
fn general_mode_no_call_site_candidate_produces_insufficient_evidence() {
    // General mode (query has no load/save/config/etc terms; "handled" triggers investigation
    // without triggering any specific lookup mode).
    // Only candidate has load terms exclusively on definition lines.
    // has_non_definition_load_candidates = false — Gate 6a never fires (no call-site to dispatch to).
    // Model answers without reading → runtime seeds read directly → evidence accepted → ToolAssisted.
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("services")).unwrap();
    fs::write(
        tmp.path().join("services").join("session_loader.py"),
        "def load_session(session_id):\n    return None\n",
    )
    .unwrap();

    let mut rt = make_runtime_in(
        vec![
            "[search_code: load_session]",
            "Sessions are handled in services/session_loader.py.",
            "Sessions are handled in services/session_loader.py.",
        ],
        tmp.path(),
    );

    let events = collect_events(
        &mut rt,
        RuntimeRequest::Submit {
            text: "Where are sessions handled?".into(),
        },
    );

    let answer_source = events.iter().find_map(|e| {
        if let RuntimeEvent::AnswerReady(src) = e {
            Some(src.clone())
        } else {
            None
        }
    });
    assert!(
        matches!(answer_source, Some(AnswerSource::ToolAssisted { .. })),
        "General mode must seed read and produce ToolAssisted: {answer_source:?}"
    );
}
