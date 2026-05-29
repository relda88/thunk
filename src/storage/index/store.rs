use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::core::error::{AppError, Result};
use crate::runtime::{ExtractedSymbol, ImportEdge};

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
        let conn =
            Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
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

    pub(crate) fn upsert_imports(
        &self,
        project_root: &str,
        edges: &[ImportEdge],
    ) -> Result<()> {
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

    pub(crate) fn lookup_imports(
        &self,
        project_root: &str,
        file: &str,
    ) -> Result<Vec<ImportEdge>> {
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
        store
            .upsert_symbols("root", &[make_symbol("a")])
            .unwrap();
        let results = store.lookup_symbol("root", "b").unwrap();
        assert!(results.is_empty(), "stale symbol must be deleted on re-upsert");
    }

    #[test]
    fn lookup_symbol_empty_for_unknown_name() {
        let store = in_memory();
        store.upsert_symbols("root", &[make_symbol("x")]).unwrap();
        let results = store.lookup_symbol("root", "nonexistent").unwrap();
        assert!(results.is_empty());
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
