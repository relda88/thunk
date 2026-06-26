use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use super::types::{ExtractedSymbol, ImportEdge};
use crate::core::error::{AppError, Result};

// deferred: typed symbol query API
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct SymbolRecord {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) file_path: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) signature: String,
    pub(crate) confidence: String,
    pub(crate) parent_scope: Option<String>,
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
                     (project_root, name, kind, file_path, line, col, signature, confidence, updated_at, parent_scope) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                        sym.parent_scope,
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

    pub(crate) fn upsert_symbols_for_file(
        &self,
        project_root: &str,
        file_path: &str,
        symbols: &[ExtractedSymbol],
    ) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "DELETE FROM index_symbols WHERE project_root = ?1 AND file_path = ?2",
                params![project_root, file_path],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        for sym in symbols {
            self.conn
                .execute(
                    "INSERT INTO index_symbols \
                     (project_root, name, kind, file_path, line, col, signature, confidence, updated_at, parent_scope) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                        sym.parent_scope,
                    ],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn upsert_imports_for_file(
        &self,
        project_root: &str,
        file_path: &str,
        edges: &[ImportEdge],
    ) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "DELETE FROM index_imports WHERE project_root = ?1 AND from_file = ?2",
                params![project_root, file_path],
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

    /// Deletes embeddings for all symbols in `file_path`. Must be called before
    /// `upsert_symbols_for_file` — the subquery depends on the symbol rows still existing.
    pub(crate) fn delete_embeddings_for_file(
        &self,
        project_root: &str,
        file_path: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM index_embeddings \
                 WHERE project_root = ?1 \
                 AND symbol_id IN (SELECT id FROM index_symbols WHERE project_root = ?1 AND file_path = ?2)",
                params![project_root, file_path],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
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
                "SELECT name, kind, file_path, line, col, signature, confidence, parent_scope \
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
                    parent_scope: row.get(7)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub(crate) fn symbols_for_file(
        &self,
        project_root: &str,
        file_path: &str,
    ) -> Result<Vec<SymbolRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, kind, file_path, line, col, signature, confidence, parent_scope \
                 FROM index_symbols WHERE project_root = ?1 AND file_path = ?2",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root, file_path], |row| {
                Ok(SymbolRecord {
                    name: row.get(0)?,
                    kind: row.get(1)?,
                    file_path: row.get(2)?,
                    line: row.get::<_, i64>(3)? as usize,
                    col: row.get::<_, i64>(4)? as usize,
                    signature: row.get(5)?,
                    confidence: row.get(6)?,
                    parent_scope: row.get(7)?,
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

    pub(crate) fn importers_of(
        &self,
        project_root: &str,
        to_file: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT from_file FROM index_imports \
                 WHERE project_root = ?1 AND to_file = ?2 LIMIT ?3",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root, to_file, limit as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub(crate) fn test_importers_of(
        &self,
        project_root: &str,
        to_file: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let candidates = self.importers_of(project_root, to_file, limit * 4)?;
        Ok(candidates
            .into_iter()
            .filter(|p| is_test_path(p))
            .take(limit)
            .collect())
    }

    pub(crate) fn all_symbols_ranked(&self, project_root: &str) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, signature FROM index_symbols \
                 WHERE project_root = ?1 \
                 ORDER BY CASE confidence \
                     WHEN 'High' THEN 0 \
                     WHEN 'Medium' THEN 1 \
                     ELSE 2 \
                 END, id",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub(crate) fn upsert_embeddings(
        &self,
        project_root: &str,
        records: &[(i64, Vec<f32>, String)],
    ) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| AppError::Storage(e.to_string()))?;
        let now = now_str();
        for (symbol_id, embedding, model_name) in records {
            let blob = crate::storage::vector::encode_embedding(embedding);
            tx.execute(
                "INSERT OR REPLACE INTO index_embeddings \
                 (project_root, symbol_id, embedding, model_name, generated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project_root, symbol_id, &blob, model_name, &now],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        tx.commit().map_err(|e| AppError::Storage(e.to_string()))
    }

    pub(crate) fn clear_embeddings(&self, project_root: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM index_embeddings WHERE project_root = ?1",
                params![project_root],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn get_embedding_model(&self, project_root: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT model_name FROM index_embeddings \
                 WHERE project_root = ?1 LIMIT 1",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(params![project_root])
            .map_err(|e| AppError::Storage(e.to_string()))?;

        match rows.next().map_err(|e| AppError::Storage(e.to_string()))? {
            Some(row) => Ok(Some(
                row.get(0).map_err(|e| AppError::Storage(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    pub(crate) fn embedding_count(&self, project_root: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM index_embeddings WHERE project_root = ?1",
                params![project_root],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    pub(crate) fn cosine_search(
        &self,
        project_root: &str,
        model_name: &str,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ie.embedding, sym.file_path \
                 FROM index_embeddings ie \
                 JOIN index_symbols sym ON ie.symbol_id = sym.id \
                 WHERE ie.project_root = ?1 AND ie.model_name = ?2",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![project_root, model_name], |row| {
                let blob: Vec<u8> = row.get(0)?;
                let file_path: String = row.get(1)?;
                Ok((blob, file_path))
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut scored: Vec<(f32, String)> = Vec::new();
        let mut dim_checked = false;
        for row in rows {
            let (blob, file_path) = row.map_err(|e| AppError::Storage(e.to_string()))?;
            let embedding = crate::storage::vector::decode_embedding(&blob);
            if !embedding.is_empty() {
                if !dim_checked {
                    if embedding.len() != query.len() {
                        return Ok(vec![]);
                    }
                    dim_checked = true;
                }
                let score = crate::storage::vector::cosine_similarity(query, &embedding);
                scored.push((score, file_path));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut seen = std::collections::HashSet::new();
        let files = scored
            .into_iter()
            .filter_map(|(_, f)| {
                if seen.insert(f.clone()) {
                    Some(f)
                } else {
                    None
                }
            })
            .take(limit)
            .collect();

        Ok(files)
    }

    #[cfg(feature = "vector-extensions")]
    pub(crate) fn try_load_vss(&self) -> bool {
        use std::path::Path;
        let candidates = ["vss0.so", "vss0.dylib", "vss0.dll"];
        for name in &candidates {
            let loaded = unsafe {
                if self.conn.load_extension_enable().is_err() {
                    false
                } else {
                    let ok = self.conn.load_extension(Path::new(name), None).is_ok();
                    let _ = self.conn.load_extension_disable();
                    ok
                }
            };
            if loaded {
                return true;
            }
        }
        false
    }
}

fn is_test_path(path: &str) -> bool {
    path.contains("tests/") || path.ends_with("_test.rs") || path.contains("/test_")
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
            parent_scope: None,
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
        assert_eq!(r.parent_scope, None);
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
    fn symbols_for_file_returns_only_matching_file() {
        let store = in_memory();
        let sym_a = ExtractedSymbol {
            name: "foo".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/foo.rs".to_string(),
            line: 1,
            col: 1,
            signature: "pub fn foo()".to_string(),
            confidence: SymbolConfidence::High,
            parent_scope: None,
        };
        let sym_b = ExtractedSymbol {
            name: "bar".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/foo.rs".to_string(),
            line: 5,
            col: 1,
            signature: "pub fn bar()".to_string(),
            confidence: SymbolConfidence::High,
            parent_scope: None,
        };
        let sym_other = ExtractedSymbol {
            name: "baz".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/bar.rs".to_string(),
            line: 1,
            col: 1,
            signature: "pub fn baz()".to_string(),
            confidence: SymbolConfidence::High,
            parent_scope: None,
        };
        store
            .upsert_symbols("root", &[sym_a, sym_b, sym_other])
            .unwrap();
        let results = store.symbols_for_file("root", "src/foo.rs").unwrap();
        assert_eq!(results.len(), 2);
        let mut names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["bar", "foo"]);
        let none = store.symbols_for_file("root", "src/bar.rs").unwrap();
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].name, "baz");
        let empty = store
            .symbols_for_file("root", "src/nonexistent.rs")
            .unwrap();
        assert!(empty.is_empty());
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
        assert!(
            all.is_empty(),
            "must not return edges for a different project root"
        );
    }

    #[test]
    fn importers_of_returns_files_that_import_target() {
        let store = in_memory();
        let edges = vec![
            ImportEdge {
                from_file: "src/a.rs".to_string(),
                to_file: "src/foo.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/b.rs".to_string(),
                to_file: "src/foo.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/c.rs".to_string(),
                to_file: "src/bar.rs".to_string(),
            },
        ];
        store.upsert_imports("root", &edges).unwrap();

        let importers = store.importers_of("root", "src/foo.rs", 10).unwrap();
        assert_eq!(importers.len(), 2);
        assert!(importers.contains(&"src/a.rs".to_string()));
        assert!(importers.contains(&"src/b.rs".to_string()));

        let none = store.importers_of("root", "src/bar.rs", 10).unwrap();
        assert_eq!(none.len(), 1);
        assert!(none.contains(&"src/c.rs".to_string()));

        let empty = store.importers_of("root", "src/unknown.rs", 10).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_importers_of_filters_to_test_paths() {
        let store = in_memory();
        let edges = vec![
            ImportEdge {
                from_file: "tests/foo.rs".to_string(),
                to_file: "src/target.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/bar_test.rs".to_string(),
                to_file: "src/target.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/test_baz.rs".to_string(),
                to_file: "src/target.rs".to_string(),
            },
            ImportEdge {
                from_file: "src/lib.rs".to_string(),
                to_file: "src/target.rs".to_string(),
            },
        ];
        store.upsert_imports("root", &edges).unwrap();

        let tests = store
            .test_importers_of("root", "src/target.rs", 10)
            .unwrap();
        assert_eq!(tests.len(), 3);
        assert!(tests.contains(&"tests/foo.rs".to_string()));
        assert!(tests.contains(&"src/bar_test.rs".to_string()));
        assert!(tests.contains(&"src/test_baz.rs".to_string()));
        assert!(!tests.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_importers_of_respects_limit() {
        let store = in_memory();
        let edges: Vec<ImportEdge> = (0..10)
            .map(|i| ImportEdge {
                from_file: format!("tests/test_{i}.rs"),
                to_file: "src/target.rs".to_string(),
            })
            .collect();
        store.upsert_imports("root", &edges).unwrap();

        let tests = store.test_importers_of("root", "src/target.rs", 3).unwrap();
        assert_eq!(tests.len(), 3);
    }

    #[test]
    fn upsert_and_get_embedding_model() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("fn_a")])
            .unwrap();
        let sym_id = {
            let mut stmt = store
                .conn
                .prepare("SELECT id FROM index_symbols WHERE project_root = 'root' LIMIT 1")
                .unwrap();
            stmt.query_row([], |r| r.get::<_, i64>(0)).unwrap()
        };
        assert!(store.get_embedding_model("root").unwrap().is_none());
        store
            .upsert_embeddings("root", &[(sym_id, vec![1.0, 0.0], "nomic".to_string())])
            .unwrap();
        assert_eq!(
            store.get_embedding_model("root").unwrap().as_deref(),
            Some("nomic")
        );
    }

    #[test]
    fn clear_embeddings_removes_all_for_project() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("fn_b")])
            .unwrap();
        let sym_id = {
            let mut stmt = store
                .conn
                .prepare("SELECT id FROM index_symbols WHERE project_root = 'root' LIMIT 1")
                .unwrap();
            stmt.query_row([], |r| r.get::<_, i64>(0)).unwrap()
        };
        store
            .upsert_embeddings("root", &[(sym_id, vec![1.0, 0.0], "m".to_string())])
            .unwrap();
        assert_eq!(store.embedding_count("root").unwrap(), 1);
        store.clear_embeddings("root").unwrap();
        assert_eq!(store.embedding_count("root").unwrap(), 0);
    }

    #[test]
    fn cosine_search_returns_best_matching_file() {
        let store = in_memory();
        store
            .upsert_symbols(
                "root",
                &[
                    make_symbol("close_fn"),
                    ExtractedSymbol {
                        name: "far_fn".to_string(),
                        kind: SymbolKind::Function,
                        file_path: "src/far.rs".to_string(),
                        line: 1,
                        col: 1,
                        signature: "pub fn far_fn()".to_string(),
                        confidence: SymbolConfidence::High,
                        parent_scope: None,
                    },
                ],
            )
            .unwrap();
        let ids: Vec<(i64, String)> = {
            let mut stmt = store
                .conn
                .prepare(
                    "SELECT id, file_path FROM index_symbols WHERE project_root = 'root' ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let query = vec![1.0f32, 0.0];
        // close_fn embedding is nearly identical to query; far_fn points away
        let embeddings = vec![
            (ids[0].0, vec![0.99f32, 0.1], "m".to_string()),
            (ids[1].0, vec![0.0f32, 1.0], "m".to_string()),
        ];
        store.upsert_embeddings("root", &embeddings).unwrap();
        let results = store.cosine_search("root", "m", &query, 5).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0], "src/foo.rs");
    }

    #[test]
    fn cosine_search_filters_by_model_name() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("fn_a")])
            .unwrap();
        let ids: Vec<(i64, String)> = {
            let mut stmt = store
                .conn
                .prepare("SELECT id, file_path FROM index_symbols WHERE project_root = 'root'")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        // Insert embeddings under two different model names.
        store
            .upsert_embeddings(
                "root",
                &[
                    (ids[0].0, vec![1.0f32, 0.0], "model_a".to_string()),
                    (ids[0].0, vec![0.0f32, 1.0], "model_b".to_string()),
                ],
            )
            .unwrap();
        // Querying with model_a should return a result (cosine = 1.0).
        let results_a = store
            .cosine_search("root", "model_a", &[1.0f32, 0.0], 5)
            .unwrap();
        assert_eq!(results_a.len(), 1);
        // Querying with model_b should return the other embedding, not both.
        let results_b = store
            .cosine_search("root", "model_b", &[1.0f32, 0.0], 5)
            .unwrap();
        assert_eq!(results_b.len(), 1);
        // Querying with an unknown model returns nothing.
        let results_none = store
            .cosine_search("root", "unknown", &[1.0f32, 0.0], 5)
            .unwrap();
        assert!(results_none.is_empty());
    }

    #[test]
    fn cosine_search_dimension_mismatch_returns_empty() {
        let store = in_memory();
        store
            .upsert_symbols("root", &[make_symbol("fn_a")])
            .unwrap();
        let ids: Vec<i64> = {
            let mut stmt = store
                .conn
                .prepare("SELECT id FROM index_symbols WHERE project_root = 'root'")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, i64>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        // Store a 3-dim embedding but query with 2-dim vector.
        store
            .upsert_embeddings("root", &[(ids[0], vec![1.0f32, 0.0, 0.5], "m".to_string())])
            .unwrap();
        let results = store.cosine_search("root", "m", &[1.0f32, 0.0], 5).unwrap();
        assert!(
            results.is_empty(),
            "dimension mismatch must return empty, not score 0"
        );
    }

    fn make_symbol_in(name: &str, file: &str) -> ExtractedSymbol {
        ExtractedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            line: 1,
            col: 1,
            signature: format!("pub fn {name}()"),
            confidence: SymbolConfidence::High,
            parent_scope: None,
        }
    }

    #[test]
    fn upsert_symbols_for_file_is_idempotent() {
        let store = in_memory();
        // Seed one symbol in a different file so the project is non-empty.
        store
            .upsert_symbols_for_file(
                "root",
                "src/bar.rs",
                &[make_symbol_in("bar_fn", "src/bar.rs")],
            )
            .unwrap();
        // Upsert for the target file twice with identical content.
        store
            .upsert_symbols_for_file(
                "root",
                "src/foo.rs",
                &[make_symbol_in("foo_fn", "src/foo.rs")],
            )
            .unwrap();
        store
            .upsert_symbols_for_file(
                "root",
                "src/foo.rs",
                &[make_symbol_in("foo_fn", "src/foo.rs")],
            )
            .unwrap();
        // Must be exactly 2 — bar's symbol + foo's symbol, no doubling.
        assert_eq!(store.symbol_count("root").unwrap(), 2);
    }

    #[test]
    fn delete_embeddings_for_file_leaves_other_files() {
        let store = in_memory();
        // Insert symbols for two different files.
        store
            .upsert_symbols_for_file(
                "root",
                "src/foo.rs",
                &[make_symbol_in("foo_fn", "src/foo.rs")],
            )
            .unwrap();
        store
            .upsert_symbols_for_file(
                "root",
                "src/bar.rs",
                &[make_symbol_in("bar_fn", "src/bar.rs")],
            )
            .unwrap();
        let (foo_id, bar_id) = {
            let mut stmt = store
                .conn
                .prepare(
                    "SELECT id, file_path FROM index_symbols \
                     WHERE project_root = 'root' ORDER BY id",
                )
                .unwrap();
            let pairs: Vec<(i64, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(pairs.len(), 2);
            (pairs[0].0, pairs[1].0)
        };
        store
            .upsert_embeddings(
                "root",
                &[
                    (foo_id, vec![1.0f32, 0.0], "m".to_string()),
                    (bar_id, vec![0.0f32, 1.0], "m".to_string()),
                ],
            )
            .unwrap();
        assert_eq!(store.embedding_count("root").unwrap(), 2);
        // Delete only foo's embeddings.
        store
            .delete_embeddings_for_file("root", "src/foo.rs")
            .unwrap();
        // bar's embedding must survive.
        assert_eq!(store.embedding_count("root").unwrap(), 1);
    }
}
