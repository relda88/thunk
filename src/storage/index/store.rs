use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use super::types::{ExtractedSymbol, ImportEdge};
use crate::core::error::{AppError, Result};

#[derive(Debug, Clone)]
pub(crate) struct SymbolRecord {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) file_path: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) signature: String,
    pub(crate) confidence: String,
}

pub(crate) struct SymbolStore {
    conn: Connection,
}

impl SymbolStore {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(Self { conn })
    }

    pub(crate) fn upsert_symbols(
        &self,
        project_root: &str,
        symbols: &[ExtractedSymbol],
    ) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "DELETE FROM index_symbols WHERE project_root = ?1",
                params![project_root],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        for sym in symbols {
            self.conn
                .execute(
                    "INSERT INTO index_symbols \
                     (project_root, name, kind, file_path, line, col, signature, confidence, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        project_root,
                        sym.name,
                        sym.kind.as_str(),
                        sym.file_path,
                        sym.line as i64,
                        sym.col as i64,
                        sym.signature,
                        sym.confidence.as_str(),
                        now,
                    ],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn upsert_imports(&self, project_root: &str, edges: &[ImportEdge]) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "DELETE FROM index_imports WHERE project_root = ?1",
                params![project_root],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        for edge in edges {
            self.conn
                .execute(
                    "INSERT INTO index_imports (project_root, from_file, to_file, updated_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![project_root, edge.from_file, edge.to_file, now],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn lookup_symbol(
        &self,
        project_root: &str,
        name: &str,
    ) -> Result<Vec<SymbolRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, kind, file_path, line, col, signature, confidence \
                 FROM index_symbols WHERE project_root = ?1 AND name = ?2",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root, name], |row| {
                Ok(SymbolRecord {
                    name: row.get(0)?,
                    kind: row.get(1)?,
                    file_path: row.get(2)?,
                    line: row.get::<_, i64>(3)? as usize,
                    col: row.get::<_, i64>(4)? as usize,
                    signature: row.get(5)?,
                    confidence: row.get(6)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub(crate) fn is_empty(&self, project_root: &str) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM index_symbols WHERE project_root = ?1",
                params![project_root],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(count == 0)
    }

    pub(crate) fn symbol_count(&self, project_root: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM index_symbols WHERE project_root = ?1",
                params![project_root],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    pub(crate) fn import_count(&self, project_root: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM index_imports WHERE project_root = ?1",
                params![project_root],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Returns the timestamp (Unix seconds as string) of the most recent build for
    /// the project, or `None` if no build has been recorded yet.
    pub(crate) fn last_build_time(&self, project_root: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT last_modified FROM file_metadata \
                 WHERE project_root = ?1 AND file_path = '' \
                 LIMIT 1",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(params![project_root])
            .map_err(|e| AppError::Storage(e.to_string()))?;

        match rows.next().map_err(|e| AppError::Storage(e.to_string()))? {
            Some(row) => {
                let ts: i64 = row.get(0).map_err(|e| AppError::Storage(e.to_string()))?;
                Ok(Some(ts.to_string()))
            }
            None => Ok(None),
        }
    }

    /// Upserts a single file metadata row. Use `file_path = ""` as a sentinel for
    /// a project-level build timestamp.
    pub(crate) fn upsert_file_metadata(
        &self,
        project_root: &str,
        file_path: &str,
        last_modified_secs: i64,
        content_hash: &str,
    ) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "INSERT INTO file_metadata \
                 (project_root, file_path, last_modified, content_hash, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(project_root, file_path) DO UPDATE SET \
                     last_modified = excluded.last_modified, \
                     content_hash  = excluded.content_hash, \
                     updated_at    = excluded.updated_at",
                params![
                    project_root,
                    file_path,
                    last_modified_secs,
                    content_hash,
                    now
                ],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn lookup_imports(&self, project_root: &str, file: &str) -> Result<Vec<ImportEdge>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT from_file, to_file FROM index_imports \
                 WHERE project_root = ?1 AND from_file = ?2",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root, file], |row| {
                Ok(ImportEdge {
                    from_file: row.get(0)?,
                    to_file: row.get(1)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }
    pub(crate) fn all_imports(&self, project_root: &str) -> Result<Vec<ImportEdge>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT from_file, to_file FROM index_imports \
                 WHERE project_root = ?1",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root], |row| {
                Ok(ImportEdge {
                    from_file: row.get(0)?,
                    to_file: row.get(1)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }
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
    use rusqlite::Connection;

    use super::*;
    use crate::runtime::{SymbolConfidence, SymbolKind};
    use crate::storage::session::schema;

    fn in_memory() -> SymbolStore {
        let conn = Connection::open_in_memory().unwrap();
        schema::initialize(&conn).unwrap();
        SymbolStore { conn }
    }

    fn make_symbol(name: &str) -> ExtractedSymbol {
        ExtractedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "src/foo.rs".to_string(),
            line: 10,
            col: 1,
            signature: format!("pub fn {name}()"),
            confidence: SymbolConfidence::High,
        }
    }

    #[test]
    fn upsert_then_lookup_returns_record() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("my_fn")])
            .unwrap();
        let results = store.lookup_symbol("root", "my_fn").unwrap();
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.name, "my_fn");
        assert_eq!(r.kind, "Function");
        assert_eq!(r.file_path, "src/foo.rs");
        assert_eq!(r.line, 10);
        assert_eq!(r.col, 1);
        assert_eq!(r.confidence, "High");
    }

    #[test]
    fn upsert_replaces_on_re_upsert() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("a"), make_symbol("b")])
            .unwrap();
        store.upsert_symbols("root", &[make_symbol("a")]).unwrap();
        let results = store.lookup_symbol("root", "b").unwrap();
        assert!(
            results.is_empty(),
            "stale symbol must be deleted on re-upsert"
        );
    }

    #[test]
    fn lookup_symbol_empty_for_unknown_name() {
        let store = in_memory();
        store.upsert_symbols("root", &[make_symbol("x")]).unwrap();
        let results = store.lookup_symbol("root", "nonexistent").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn is_empty_true_before_upsert() {
        let store = in_memory();
        assert!(store.is_empty("root").unwrap());
    }

    #[test]
    fn is_empty_false_after_upsert() {
        let store = in_memory();
        store.upsert_symbols("root", &[make_symbol("a")]).unwrap();
        assert!(!store.is_empty("root").unwrap());
    }

    #[test]
    fn symbol_count_returns_correct_count() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("a"), make_symbol("b")])
            .unwrap();
        assert_eq!(store.symbol_count("root").unwrap(), 2);
    }

    #[test]
    fn import_count_returns_correct_count() {
        let store = in_memory();
        let edges = vec![ImportEdge {
            from_file: "src/a.rs".to_string(),
            to_file: "src/b.rs".to_string(),
        }];
        store.upsert_imports("root", &edges).unwrap();
        assert_eq!(store.import_count("root").unwrap(), 1);
    }

    #[test]
    fn last_build_time_none_before_any_metadata() {
        let store = in_memory();
        assert!(store.last_build_time("root").unwrap().is_none());
    }

    #[test]
    fn upsert_file_metadata_and_last_build_time_roundtrip() {
        let store = in_memory();
        store
            .upsert_file_metadata("root", "", 1_700_000_000, "")
            .unwrap();
        let ts = store.last_build_time("root").unwrap();
        assert_eq!(ts.as_deref(), Some("1700000000"));
    }

    #[test]
    fn upsert_file_metadata_replaces_on_conflict() {
        let store = in_memory();
        store.upsert_file_metadata("root", "", 100, "h1").unwrap();
        store.upsert_file_metadata("root", "", 200, "h2").unwrap();
        let ts = store.last_build_time("root").unwrap();
        assert_eq!(ts.as_deref(), Some("200"));
    }

    #[test]
    fn all_imports_returns_all_edges_for_project() {
        let store = in_memory();
        let edges = vec![
            ImportEdge {
                from_file: "src/a.rs".to_string(),
                to_file: "src/b.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/c.rs".to_string(),
                to_file: "src/d.rs".to_string(),
            },
        ];
        store.upsert_imports("root", &edges).unwrap();
        let all = store.all_imports("root").unwrap();
        assert_eq!(all.len(), 2);
        let froms: Vec<&str> = all.iter().map(|e| e.from_file.as_str()).collect();
        assert!(froms.contains(&"src/a.rs"));
        assert!(froms.contains(&"src/c.rs"));
    }

    #[test]
    fn all_imports_empty_for_different_project() {
        let store = in_memory();
        let edges = vec![ImportEdge {
            from_file: "src/a.rs".to_string(),
            to_file: "src/b.rs".to_string(),
        }];
        store.upsert_imports("root1", &edges).unwrap();
        let all = store.all_imports("root2").unwrap();
        assert!(all.is_empty(), "must not return edges for a different project root");
    }

    #[test]
    fn upsert_imports_and_lookup_roundtrip() {
        let store = in_memory();
        let edges = vec![
            ImportEdge {
                from_file: "src/a.rs".to_string(),
                to_file: "src/b.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/a.rs".to_string(),
                to_file: "src/c.rs".to_string(),
            },
        ];
        store.upsert_imports("root", &edges).unwrap();
        let results = store.lookup_imports("root", "src/a.rs").unwrap();
        assert_eq!(results.len(), 2);
        let targets: Vec<&str> = results.iter().map(|e| e.to_file.as_str()).collect();
        assert!(targets.contains(&"src/b.rs"));
        assert!(targets.contains(&"src/c.rs"));
    }
}
