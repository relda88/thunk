pub(crate) const CURRENT_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS personal_memories (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    text             TEXT NOT NULL,
    category         TEXT NOT NULL,
    scope            TEXT,
    salience         REAL NOT NULL DEFAULT 1.0,
    embedding        BLOB,
    model_name       TEXT,
    source           TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    last_recalled_at TEXT
);
";

pub(crate) fn initialize(conn: &rusqlite::Connection) -> crate::core::error::Result<()> {
    conn.execute_batch(SCHEMA)
        .map_err(|e| crate::core::error::AppError::Storage(e.to_string()))?;
    conn.pragma_update(None, "user_version", CURRENT_VERSION)
        .map_err(|e| crate::core::error::AppError::Storage(e.to_string()))?;
    Ok(())
}

/// Returns the path to ~/.thunk/memory.db, or None if HOME is unset.
pub(crate) fn memory_db_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        let mut p = std::path::PathBuf::from(h);
        p.push(".thunk");
        p.push("memory.db");
        p
    })
}
