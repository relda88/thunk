use rusqlite::Connection;

use crate::core::error::{AppError, Result};

const CURRENT_VERSION: i32 = 8;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS sessions (
        id                TEXT PRIMARY KEY,
        project_root      TEXT,
        created_at        INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL,
        msg_count         INTEGER NOT NULL DEFAULT 0,
        last_read_file    TEXT,
        last_search_query TEXT,
        last_search_scope TEXT
    );

    CREATE TABLE IF NOT EXISTS session_messages (
        session_id  TEXT NOT NULL,
        seq         INTEGER NOT NULL,
        role        TEXT NOT NULL,
        content     TEXT NOT NULL,
        PRIMARY KEY (session_id, seq)
    );

    CREATE INDEX IF NOT EXISTS idx_sessions_updated
        ON sessions(updated_at DESC);

    CREATE INDEX IF NOT EXISTS idx_session_messages_lookup
        ON session_messages(session_id, seq);

    CREATE TABLE IF NOT EXISTS index_symbols (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        project_root  TEXT NOT NULL,
        name          TEXT NOT NULL,
        kind          TEXT NOT NULL,
        file_path     TEXT NOT NULL,
        line          INTEGER NOT NULL,
        col           INTEGER NOT NULL,
        signature     TEXT NOT NULL,
        confidence    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        parent_scope  TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_symbols_project_name
        ON index_symbols (project_root, name);
    CREATE INDEX IF NOT EXISTS idx_symbols_project_file
        ON index_symbols (project_root, file_path);

    CREATE TABLE IF NOT EXISTS index_imports (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        project_root  TEXT NOT NULL,
        from_file     TEXT NOT NULL,
        to_file       TEXT NOT NULL,
        updated_at    TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_imports_project_source
        ON index_imports (project_root, from_file);

    CREATE TABLE IF NOT EXISTS file_metadata (
        project_root  TEXT NOT NULL,
        file_path     TEXT NOT NULL,
        last_modified INTEGER NOT NULL,
        content_hash  TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        PRIMARY KEY (project_root, file_path)
    );

    CREATE TABLE IF NOT EXISTS plans (
        id           TEXT PRIMARY KEY,
        session_id   TEXT NOT NULL,
        project_root TEXT NOT NULL,
        goal         TEXT NOT NULL,
        status       TEXT NOT NULL DEFAULT 'draft',
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS plan_tasks (
        id             TEXT PRIMARY KEY,
        plan_id        TEXT NOT NULL REFERENCES plans(id),
        session_id     TEXT NOT NULL,
        project_root   TEXT NOT NULL,
        step_number    INTEGER NOT NULL,
        title          TEXT NOT NULL,
        description    TEXT NOT NULL DEFAULT '',
        status         TEXT NOT NULL DEFAULT 'pending',
        result_summary TEXT NOT NULL DEFAULT '',
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS index_embeddings (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        project_root TEXT NOT NULL,
        symbol_id    INTEGER NOT NULL,
        embedding    BLOB NOT NULL,
        model_name   TEXT NOT NULL,
        generated_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_embeddings_project_model
        ON index_embeddings (project_root, model_name);
";

pub(crate) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
        .map_err(|e| AppError::Storage(e.to_string()))?;

    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| AppError::Storage(e.to_string()))?;

    if version < 2 && !has_column(conn, "sessions", "project_root")? {
        conn.execute("ALTER TABLE sessions ADD COLUMN project_root TEXT", [])
            .map_err(|e| AppError::Storage(e.to_string()))?;
    }

    if version < 3 {
        if !has_column(conn, "sessions", "last_read_file")? {
            conn.execute("ALTER TABLE sessions ADD COLUMN last_read_file TEXT", [])
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        if !has_column(conn, "sessions", "last_search_query")? {
            conn.execute("ALTER TABLE sessions ADD COLUMN last_search_query TEXT", [])
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        if !has_column(conn, "sessions", "last_search_scope")? {
            conn.execute("ALTER TABLE sessions ADD COLUMN last_search_scope TEXT", [])
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
    }

    if version < 4 {
        // net-new tables — CREATE TABLE IF NOT EXISTS in SCHEMA handles migration
    }

    if version < 5 {
        // file_metadata table — CREATE TABLE IF NOT EXISTS in SCHEMA handles migration
    }

    if version < 6 {
        // plans and plan_tasks tables — CREATE TABLE IF NOT EXISTS in SCHEMA handles migration
    }

    if version < 7 {
        if !has_column(conn, "index_symbols", "parent_scope")? {
            conn.execute("ALTER TABLE index_symbols ADD COLUMN parent_scope TEXT", [])
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
    }

    if version < 8 {
        // index_embeddings table — CREATE TABLE IF NOT EXISTS in SCHEMA handles migration
    }

    if version < CURRENT_VERSION {
        conn.pragma_update(None, "user_version", CURRENT_VERSION)
            .map_err(|e| AppError::Storage(e.to_string()))?;
    }

    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| AppError::Storage(e.to_string()))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| AppError::Storage(e.to_string()))?;

    while let Some(row) = rows.next().map_err(|e| AppError::Storage(e.to_string()))? {
        let name: String = row.get(1).map_err(|e| AppError::Storage(e.to_string()))?;
        if name == column {
            return Ok(true);
        }
    }

    Ok(false)
}
