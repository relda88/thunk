use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::app::{AppError, Result};

use super::schema;
use super::types::{generate_session_id, now_ms, SavedSession, SessionMeta, StoredMessage};

pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    /// Opens (or creates) a session database at the given path.
    /// The parent directory must already exist — callers should use AppPaths::ensure_runtime_dirs first.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
        schema::initialize(&conn)?;
        Ok(Self { conn })
    }

    /// Creates a new empty session and returns its metadata.
    pub fn create(&self, project_root: &Path) -> Result<SessionMeta> {
        let id = generate_session_id();
        let now = now_ms();
        let project_root = project_root.to_string_lossy().into_owned();
        self.conn
            .execute(
                "INSERT INTO sessions (id, project_root, created_at, updated_at, msg_count)
                 VALUES (?1, ?2, ?3, ?3, 0)",
                params![id, project_root, now as i64],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        self.require_meta(&id)
    }

    /// Persists messages and anchor state for an existing session. Replaces any previously saved messages.
    /// Returns updated metadata with the new message count and timestamp.
    pub fn save(
        &self,
        id: &str,
        messages: &[StoredMessage],
        last_read_file: Option<&str>,
        last_search_query: Option<&str>,
        last_search_scope: Option<&str>,
    ) -> Result<SessionMeta> {
        let now = now_ms();
        let count = messages.len();

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        tx.execute(
            "UPDATE sessions SET updated_at = ?2, msg_count = ?3, last_read_file = ?4, last_search_query = ?5, last_search_scope = ?6 WHERE id = ?1",
            params![id, now as i64, count as i64, last_read_file, last_search_query, last_search_scope],
        )
        .map_err(|e| AppError::Storage(e.to_string()))?;

        tx.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            params![id],
        )
        .map_err(|e| AppError::Storage(e.to_string()))?;

        for (seq, msg) in messages.iter().enumerate() {
            tx.execute(
                "INSERT INTO session_messages (session_id, seq, role, content)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, seq as i64, msg.role, msg.content],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        }

        tx.commit().map_err(|e| AppError::Storage(e.to_string()))?;

        self.require_meta(id)
    }

    /// Loads a session by ID. Returns None if the ID does not exist.
    pub fn load(&self, id: &str) -> Result<Option<SavedSession>> {
        let Some(meta) = self.load_meta(id)? else {
            return Ok(None);
        };

        let messages = self
            .conn
            .prepare(
                "SELECT role, content
                 FROM session_messages
                 WHERE session_id = ?1
                 ORDER BY seq ASC",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?
            .query_map(params![id], |row| {
                Ok(StoredMessage {
                    role: row.get(0)?,
                    content: row.get(1)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        Ok(Some(SavedSession { meta, messages }))
    }

    /// Loads the most recently updated session. Returns None if there are no sessions.
    pub fn load_most_recent(&self) -> Result<Option<SavedSession>> {
        let id = self
            .conn
            .query_row(
                "SELECT id FROM sessions ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        match id {
            Some(id) => self.load(&id),
            None => Ok(None),
        }
    }

    /// Loads the most recently updated session for the given project root.
    /// Returns None if no session exists for that project.
    pub fn load_most_recent_for_project(&self, project_root: &str) -> Result<Option<SavedSession>> {
        let id = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE project_root = ?1 ORDER BY updated_at DESC LIMIT 1",
                params![project_root],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        match id {
            Some(id) => self.load(&id),
            None => Ok(None),
        }
    }

    /// Lists all sessions ordered by most recently updated.
    pub fn list(&self) -> Result<Vec<SessionMeta>> {
        self.conn
            .prepare(
                "SELECT id, project_root, created_at, updated_at, msg_count,
                        last_read_file, last_search_query, last_search_scope
                 FROM sessions
                 ORDER BY updated_at DESC",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?
            .query_map([], |row| {
                Ok(SessionMeta {
                    id: row.get(0)?,
                    project_root: row.get(1)?,
                    created_at: row.get::<_, i64>(2)? as u64,
                    updated_at: row.get::<_, i64>(3)? as u64,
                    message_count: row.get::<_, i64>(4)? as usize,
                    last_read_file: row.get(5)?,
                    last_search_query: row.get(6)?,
                    last_search_scope: row.get(7)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Deletes a session and all its messages.
    pub fn delete(&self, id: &str) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        tx.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            params![id],
        )
        .map_err(|e| AppError::Storage(e.to_string()))?;

        tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(|e| AppError::Storage(e.to_string()))?;

        tx.commit().map_err(|e| AppError::Storage(e.to_string()))
    }

    fn load_meta(&self, id: &str) -> Result<Option<SessionMeta>> {
        self.conn
            .query_row(
                "SELECT id, project_root, created_at, updated_at, msg_count,
                        last_read_file, last_search_query, last_search_scope
                 FROM sessions WHERE id = ?1",
                params![id],
                |row| {
                    Ok(SessionMeta {
                        id: row.get(0)?,
                        project_root: row.get(1)?,
                        created_at: row.get::<_, i64>(2)? as u64,
                        updated_at: row.get::<_, i64>(3)? as u64,
                        message_count: row.get::<_, i64>(4)? as usize,
                        last_read_file: row.get(5)?,
                        last_search_query: row.get(6)?,
                        last_search_scope: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    fn require_meta(&self, id: &str) -> Result<SessionMeta> {
        self.load_meta(id)?
            .ok_or_else(|| AppError::Storage(format!("session not found: {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory() -> SessionStore {
        let conn = Connection::open_in_memory().unwrap();
        schema::initialize(&conn).unwrap();
        SessionStore { conn }
    }

    #[test]
    fn create_and_list() {
        let store = in_memory();
        let a = store.create(Path::new("/tmp/project-a")).unwrap();
        let b = store.create(Path::new("/tmp/project-b")).unwrap();
        let sessions = store.list().unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == a.id));
        assert!(sessions.iter().any(|s| s.id == b.id));
        assert_eq!(a.project_root.as_deref(), Some("/tmp/project-a"));
        assert_eq!(b.project_root.as_deref(), Some("/tmp/project-b"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let store = in_memory();
        let meta = store.create(Path::new("/tmp/project")).unwrap();

        let messages = vec![
            StoredMessage {
                role: "user".into(),
                content: "hello".into(),
            },
            StoredMessage {
                role: "assistant".into(),
                content: "hi there".into(),
            },
        ];
        let saved = store.save(&meta.id, &messages, None, None, None).unwrap();
        assert_eq!(saved.message_count, 2);
        assert_eq!(saved.project_root.as_deref(), Some("/tmp/project"));

        let loaded = store.load(&meta.id).unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role, "user");
        assert_eq!(loaded.messages[1].content, "hi there");
        assert_eq!(loaded.meta.project_root.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn save_replaces_existing_messages() {
        let store = in_memory();
        let meta = store.create(Path::new("/tmp/project")).unwrap();

        store
            .save(
                &meta.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "first".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        store
            .save(
                &meta.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "replaced".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        let loaded = store.load(&meta.id).unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, "replaced");
    }

    #[test]
    fn load_most_recent_returns_latest() {
        let store = in_memory();
        let a = store.create(Path::new("/tmp/project-a")).unwrap();
        let b = store.create(Path::new("/tmp/project-b")).unwrap();

        // Save to b last so it is most recent
        store
            .save(
                &a.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "a".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();
        store
            .save(
                &b.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "b".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        let recent = store.load_most_recent().unwrap().unwrap();
        assert_eq!(recent.meta.id, b.id);
        assert_eq!(recent.meta.project_root.as_deref(), Some("/tmp/project-b"));
    }

    #[test]
    fn load_most_recent_for_project_returns_only_matching_project() {
        let store = in_memory();
        let a = store.create(Path::new("/tmp/project-a")).unwrap();
        let b = store.create(Path::new("/tmp/project-b")).unwrap();

        store
            .save(
                &a.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "a".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();
        // Save to b last so it is globally most recent
        store
            .save(
                &b.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "b".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        let result = store
            .load_most_recent_for_project("/tmp/project-a")
            .unwrap()
            .unwrap();
        assert_eq!(result.meta.id, a.id);
        assert_eq!(result.messages[0].content, "a");
    }

    #[test]
    fn load_most_recent_for_project_returns_none_when_no_match() {
        let store = in_memory();
        store.create(Path::new("/tmp/project-a")).unwrap();

        let result = store
            .load_most_recent_for_project("/tmp/other-project")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn delete_removes_session_and_messages() {
        let store = in_memory();
        let meta = store.create(Path::new("/tmp/project")).unwrap();
        store
            .save(
                &meta.id,
                &[StoredMessage {
                    role: "user".into(),
                    content: "gone".into(),
                }],
                None,
                None,
                None,
            )
            .unwrap();

        store.delete(&meta.id).unwrap();

        assert!(store.load(&meta.id).unwrap().is_none());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn anchors_saved_and_loaded_with_session() {
        let store = in_memory();
        let meta = store.create(Path::new("/tmp/project")).unwrap();

        store
            .save(
                &meta.id,
                &[],
                Some("src/lib.rs"),
                Some("fn main"),
                Some("src/"),
            )
            .unwrap();

        let loaded = store.load(&meta.id).unwrap().unwrap();
        assert_eq!(loaded.meta.last_read_file.as_deref(), Some("src/lib.rs"));
        assert_eq!(loaded.meta.last_search_query.as_deref(), Some("fn main"));
        assert_eq!(loaded.meta.last_search_scope.as_deref(), Some("src/"));
    }

    #[test]
    fn missing_anchor_data_defaults_to_none() {
        let store = in_memory();
        let meta = store.create(Path::new("/tmp/project")).unwrap();

        store.save(&meta.id, &[], None, None, None).unwrap();

        let loaded = store.load(&meta.id).unwrap().unwrap();
        assert_eq!(loaded.meta.last_read_file, None);
        assert_eq!(loaded.meta.last_search_query, None);
        assert_eq!(loaded.meta.last_search_scope, None);
    }

    #[test]
    fn anchor_columns_default_to_null_on_v2_schema_migration() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE sessions (
                id           TEXT PRIMARY KEY,
                project_root TEXT,
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL,
                msg_count    INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE session_messages (
                session_id TEXT NOT NULL,
                seq        INTEGER NOT NULL,
                role       TEXT NOT NULL,
                content    TEXT NOT NULL,
                PRIMARY KEY (session_id, seq)
            );
            CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC);
            CREATE INDEX idx_session_messages_lookup ON session_messages(session_id, seq);
            PRAGMA user_version = 2;
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_root, created_at, updated_at, msg_count)
             VALUES ('s1', '/tmp/project', 1, 1, 0)",
            [],
        )
        .unwrap();

        schema::initialize(&conn).unwrap();

        let store = SessionStore { conn };
        let loaded = store.load("s1").unwrap().unwrap();
        assert_eq!(loaded.meta.last_read_file, None);
        assert_eq!(loaded.meta.last_search_query, None);
        assert_eq!(loaded.meta.last_search_scope, None);
    }

    #[test]
    fn load_unknown_id_returns_none() {
        let store = in_memory();
        assert!(store.load("does-not-exist").unwrap().is_none());
    }
}
