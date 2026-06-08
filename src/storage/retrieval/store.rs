use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::core::error::{AppError, Result};

pub(crate) struct RetrievalLogEntry {
    pub(crate) project_root: String,
    pub(crate) strategy: String,
    pub(crate) candidates_found: usize,
    pub(crate) reads_accepted: usize,
    pub(crate) evidence_outcome: String,
    pub(crate) hops_taken: usize,
    pub(crate) vector_augmented: bool,
}

pub(crate) struct RetrievalLogStore {
    conn: Connection,
}

impl RetrievalLogStore {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
        // Ensure the table exists even when schema::initialize hasn't run yet
        // (e.g. in tests that construct Runtime directly without a SessionStore).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS retrieval_log (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                project_root     TEXT NOT NULL,
                recorded_at      TEXT NOT NULL,
                strategy         TEXT NOT NULL,
                candidates_found INTEGER NOT NULL,
                reads_accepted   INTEGER NOT NULL,
                evidence_outcome TEXT NOT NULL,
                hops_taken       INTEGER NOT NULL DEFAULT 0,
                vector_augmented INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_retrieval_log_project
                ON retrieval_log (project_root, id DESC);",
        )
        .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(Self { conn })
    }

    pub(crate) fn insert(&self, entry: &RetrievalLogEntry) -> Result<()> {
        let recorded_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());

        self.conn
            .execute(
                "INSERT INTO retrieval_log \
                 (project_root, recorded_at, strategy, candidates_found, reads_accepted, \
                  evidence_outcome, hops_taken, vector_augmented) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.project_root,
                    recorded_at,
                    entry.strategy,
                    entry.candidates_found as i64,
                    entry.reads_accepted as i64,
                    entry.evidence_outcome,
                    entry.hops_taken as i64,
                    if entry.vector_augmented { 1i64 } else { 0i64 },
                ],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let _ = self.conn.execute(
            "DELETE FROM retrieval_log \
             WHERE project_root = ?1 \
             AND id NOT IN (\
                 SELECT id FROM retrieval_log \
                 WHERE project_root = ?1 \
                 ORDER BY id DESC \
                 LIMIT 1000\
             )",
            params![entry.project_root],
        );

        Ok(())
    }

    pub(crate) fn last_n(&self, project_root: &str, n: usize) -> Result<Vec<RetrievalLogEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT project_root, strategy, candidates_found, reads_accepted, \
                 evidence_outcome, hops_taken, vector_augmented \
                 FROM retrieval_log \
                 WHERE project_root = ?1 \
                 ORDER BY id DESC \
                 LIMIT ?2",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root, n as i64], |row| {
                Ok(RetrievalLogEntry {
                    project_root: row.get(0)?,
                    strategy: row.get(1)?,
                    candidates_found: row.get::<_, i64>(2)? as usize,
                    reads_accepted: row.get::<_, i64>(3)? as usize,
                    evidence_outcome: row.get(4)?,
                    hops_taken: row.get::<_, i64>(5)? as usize,
                    vector_augmented: row.get::<_, i64>(6)? != 0,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(entries)
    }

    /// Returns the fraction of the last `n` entries (for this project) where
    /// `reads_accepted > 0`. Returns `None` if fewer than `n` entries exist.
    pub(crate) fn recent_hit_rate(&self, project_root: &str, n: usize) -> Result<Option<f32>> {
        let total: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM (\
                     SELECT id FROM retrieval_log \
                     WHERE project_root = ?1 \
                     ORDER BY id DESC \
                     LIMIT ?2\
                 )",
                params![project_root, n as i64],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        if total < n as i64 {
            return Ok(None);
        }

        let hits: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM (\
                     SELECT id, reads_accepted FROM retrieval_log \
                     WHERE project_root = ?1 \
                     ORDER BY id DESC \
                     LIMIT ?2\
                 ) WHERE reads_accepted > 0",
                params![project_root, n as i64],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        Ok(Some(hits as f32 / total as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn make_store() -> RetrievalLogStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE retrieval_log (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                project_root     TEXT NOT NULL,
                recorded_at      TEXT NOT NULL,
                strategy         TEXT NOT NULL,
                candidates_found INTEGER NOT NULL,
                reads_accepted   INTEGER NOT NULL,
                evidence_outcome TEXT NOT NULL,
                hops_taken       INTEGER NOT NULL DEFAULT 0,
                vector_augmented INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        RetrievalLogStore { conn }
    }

    fn entry(
        project_root: &str,
        reads_accepted: usize,
        candidates_found: usize,
    ) -> RetrievalLogEntry {
        RetrievalLogEntry {
            project_root: project_root.to_string(),
            strategy: "General".to_string(),
            candidates_found,
            reads_accepted,
            evidence_outcome: "met".to_string(),
            hops_taken: 0,
            vector_augmented: false,
        }
    }

    #[test]
    fn insert_and_last_n_roundtrip() {
        let store = make_store();
        store.insert(&entry("/proj", 1, 3)).unwrap();
        store.insert(&entry("/proj", 0, 2)).unwrap();
        let rows = store.last_n("/proj", 10).unwrap();
        assert_eq!(rows.len(), 2);
        // last_n returns newest first
        assert_eq!(rows[0].reads_accepted, 0);
        assert_eq!(rows[1].reads_accepted, 1);
    }

    #[test]
    fn prune_caps_at_1000() {
        let store = make_store();
        for i in 0..1005usize {
            store.insert(&entry("/proj", i % 2, 3)).unwrap();
        }
        let rows = store.last_n("/proj", 2000).unwrap();
        assert_eq!(rows.len(), 1000);
    }

    #[test]
    fn recent_hit_rate_returns_none_below_threshold() {
        let store = make_store();
        store.insert(&entry("/proj", 1, 3)).unwrap();
        // Only 1 entry, threshold is 5
        assert!(store.recent_hit_rate("/proj", 5).unwrap().is_none());
    }

    #[test]
    fn recent_hit_rate_correct() {
        let store = make_store();
        // Insert 5: 2 with hits, 3 without
        store.insert(&entry("/proj", 1, 3)).unwrap();
        store.insert(&entry("/proj", 0, 2)).unwrap();
        store.insert(&entry("/proj", 0, 2)).unwrap();
        store.insert(&entry("/proj", 1, 3)).unwrap();
        store.insert(&entry("/proj", 0, 2)).unwrap();
        let rate = store.recent_hit_rate("/proj", 5).unwrap().unwrap();
        assert!((rate - 0.4).abs() < 0.001, "expected 0.4, got {rate}");
    }
}
