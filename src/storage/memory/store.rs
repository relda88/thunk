use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};

use super::types::{MemoryFact, MemorySource};
use crate::core::error::{AppError, Result};

pub(crate) struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
        super::schema::initialize(&conn)?;
        Ok(Self { conn })
    }

    pub(crate) fn upsert_fact(
        &self,
        text: &str,
        category: &str,
        scope: Option<&str>,
        salience: f64,
        embedding: Option<&[u8]>,
        model_name: Option<&str>,
        source: MemorySource,
    ) -> Result<i64> {
        let now = now_str();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO personal_memories \
                 (text, category, scope, salience, embedding, model_name, source, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    text,
                    category,
                    scope,
                    salience,
                    embedding,
                    model_name,
                    source.as_str(),
                    now
                ],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn delete_fact(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM personal_memories WHERE id = ?1", params![id])
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn get_fact(&self, id: i64) -> Result<Option<MemoryFact>> {
        self.conn
            .query_row(
                "SELECT id, text, category, scope, salience, embedding, model_name, source, \
                 created_at, updated_at, last_recalled_at \
                 FROM personal_memories WHERE id = ?1",
                params![id],
                row_to_fact,
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    pub(crate) fn list_facts(&self, scope: Option<&str>) -> Result<Vec<MemoryFact>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, text, category, scope, salience, embedding, model_name, source, \
                 created_at, updated_at, last_recalled_at \
                 FROM personal_memories \
                 WHERE (?1 IS NULL OR scope IS NULL OR scope = ?1)",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![scope], row_to_fact)
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut facts = Vec::new();
        for row in rows {
            facts.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(facts)
    }

    /// Facts last recalled before `before` (a Unix timestamp string), or never
    /// recalled (NULL `last_recalled_at`). Scope filter follows `list_facts`.
    pub(crate) fn stale_facts(&self, scope: Option<&str>, before: &str) -> Result<Vec<MemoryFact>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, text, category, scope, salience, embedding, model_name, source, \
                 created_at, updated_at, last_recalled_at \
                 FROM personal_memories \
                 WHERE (?1 IS NULL OR scope IS NULL OR scope = ?1) \
                 AND (last_recalled_at IS NULL OR last_recalled_at < ?2)",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![scope, before], row_to_fact)
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut facts = Vec::new();
        for row in rows {
            facts.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(facts)
    }

    pub(crate) fn update_last_recalled(&self, id: i64) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "UPDATE personal_memories SET last_recalled_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn cosine_recall(
        &self,
        query: &[f32],
        scope: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryFact>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, text, category, scope, salience, embedding, model_name, source, \
                 created_at, updated_at, last_recalled_at \
                 FROM personal_memories \
                 WHERE (?1 IS NULL OR scope IS NULL OR scope = ?1)",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![scope], row_to_fact)
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut scored: Vec<(f32, MemoryFact)> = Vec::new();
        let mut dim_checked = false;
        for row in rows {
            let fact = row.map_err(|e| AppError::Storage(e.to_string()))?;
            let blob = match &fact.embedding {
                Some(b) if !b.is_empty() => b.clone(),
                _ => continue,
            };
            let embedding = crate::storage::vector::decode_embedding(&blob);
            if !embedding.is_empty() {
                if !dim_checked {
                    if embedding.len() != query.len() {
                        return Ok(vec![]);
                    }
                    dim_checked = true;
                }
                let score = crate::storage::vector::cosine_similarity(query, &embedding);
                scored.push((score, fact));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let facts = scored.into_iter().map(|(_, f)| f).take(limit).collect();
        Ok(facts)
    }
}

fn row_to_fact(row: &Row) -> rusqlite::Result<MemoryFact> {
    let source: String = row.get(7)?;
    Ok(MemoryFact {
        id: row.get(0)?,
        text: row.get(1)?,
        category: row.get(2)?,
        scope: row.get(3)?,
        salience: row.get(4)?,
        embedding: row.get(5)?,
        model_name: row.get(6)?,
        source: MemorySource::from_str(&source),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        last_recalled_at: row.get(10)?,
    })
}

fn now_str() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::vector::encode_embedding;

    fn store() -> MemoryStore {
        let conn = Connection::open_in_memory().unwrap();
        super::super::schema::initialize(&conn).unwrap();
        MemoryStore { conn }
    }

    #[test]
    fn upsert_and_get_fact() {
        let s = store();
        let id = s
            .upsert_fact(
                "prefers rust",
                "preference",
                Some("proj_a"),
                2.5,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();
        let fact = s.get_fact(id).unwrap().unwrap();
        assert_eq!(fact.id, id);
        assert_eq!(fact.text, "prefers rust");
        assert_eq!(fact.category, "preference");
        assert_eq!(fact.scope.as_deref(), Some("proj_a"));
        assert!((fact.salience - 2.5).abs() < 1e-9);
        assert!(fact.embedding.is_none());
        assert!(matches!(fact.source, MemorySource::User));
    }

    #[test]
    fn list_facts_scope_filter() {
        let s = store();
        s.upsert_fact(
            "global fact",
            "identity",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();
        s.upsert_fact(
            "proj_a fact",
            "project",
            Some("proj_a"),
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

        let in_b = s.list_facts(Some("proj_b")).unwrap();
        assert_eq!(in_b.len(), 1);
        assert_eq!(in_b[0].text, "global fact");

        let in_a = s.list_facts(Some("proj_a")).unwrap();
        assert_eq!(in_a.len(), 2);
    }

    #[test]
    fn stale_facts_returns_never_recalled() {
        let s = store();
        s.upsert_fact(
            "never recalled",
            "misc",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

        let result = s.stale_facts(None, "9999999999").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "never recalled");
        assert!(result[0].last_recalled_at.is_none());
    }

    #[test]
    fn stale_facts_excludes_recently_recalled() {
        let s = store();
        let id = s
            .upsert_fact(
                "recently recalled",
                "misc",
                None,
                1.0,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();
        // Set last_recalled_at to "now" (current Unix epoch seconds).
        s.update_last_recalled(id).unwrap();

        // Cutoff is in the past, so a now-recalled fact is not stale.
        let result = s.stale_facts(None, "0").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn stale_facts_scope_filter() {
        let s = store();
        s.upsert_fact(
            "global fact",
            "identity",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();
        s.upsert_fact(
            "proj fact",
            "project",
            Some("proj"),
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();

        // Both facts are never recalled, so the cutoff admits both before scope filtering.
        let in_other = s.stale_facts(Some("proj_b"), "9999999999").unwrap();
        assert_eq!(in_other.len(), 1);
        assert_eq!(in_other[0].text, "global fact");

        let in_proj = s.stale_facts(Some("proj"), "9999999999").unwrap();
        assert_eq!(in_proj.len(), 2);
    }

    #[test]
    fn cosine_recall_skips_null_embedding() {
        let s = store();
        s.upsert_fact(
            "no embedding",
            "misc",
            None,
            1.0,
            None,
            None,
            MemorySource::User,
        )
        .unwrap();
        let result = s.cosine_recall(&[0.1, 0.2, 0.3], None, 5).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn cosine_recall_scope_boundary() {
        let s = store();
        let emb = encode_embedding(&[1.0, 0.0, 0.0]);
        s.upsert_fact(
            "proj_a secret",
            "project",
            Some("proj_a"),
            1.0,
            Some(&emb),
            Some("test-model"),
            MemorySource::User,
        )
        .unwrap();

        let result = s
            .cosine_recall(&[1.0, 0.0, 0.0], Some("proj_b"), 5)
            .unwrap();
        assert!(result.is_empty());
    }
}
