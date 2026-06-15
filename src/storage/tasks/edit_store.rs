use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::core::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StepStatus {
    Pending,
    Applied,
    Verified,
    Failed,
    RolledBack,
}

impl StepStatus {
    fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Applied => "applied",
            StepStatus::Verified => "verified",
            StepStatus::Failed => "failed",
            StepStatus::RolledBack => "rolled_back",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "applied" => StepStatus::Applied,
            "verified" => StepStatus::Verified,
            "failed" => StepStatus::Failed,
            "rolled_back" => StepStatus::RolledBack,
            _ => StepStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SequenceStatus {
    Planning,
    Approved,
    InProgress,
    Completed,
    Failed,
}

impl SequenceStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SequenceStatus::Planning => "planning",
            SequenceStatus::Approved => "approved",
            SequenceStatus::InProgress => "in_progress",
            SequenceStatus::Completed => "completed",
            SequenceStatus::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "approved" => SequenceStatus::Approved,
            "in_progress" => SequenceStatus::InProgress,
            "completed" => SequenceStatus::Completed,
            "failed" => SequenceStatus::Failed,
            _ => SequenceStatus::Planning,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EditStep {
    pub(crate) id: String,
    pub(crate) sequence_id: String,
    pub(crate) position: usize,
    pub(crate) file: PathBuf,
    pub(crate) search: String,
    pub(crate) replace: String,
    pub(crate) verification_cmd: Option<String>,
    pub(crate) status: StepStatus,
    pub(crate) generated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct EditSequence {
    pub(crate) id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) goal: String,
    pub(crate) steps: Vec<EditStep>,
    pub(crate) current_idx: usize,
    pub(crate) status: SequenceStatus,
    pub(crate) snapshot_ref: Option<String>,
}

pub(crate) struct EditSequenceStore {
    conn: Connection,
}

impl EditSequenceStore {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(Self { conn })
    }

    pub(crate) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub(crate) fn create_sequence(&self, sequence: &EditSequence) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO edit_sequences (id, task_id, goal, current_idx, status, snapshot_ref)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    sequence.id,
                    sequence.task_id,
                    sequence.goal,
                    sequence.current_idx as i64,
                    sequence.status.as_str(),
                    sequence.snapshot_ref,
                ],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        for step in &sequence.steps {
            self.conn
                .execute(
                    "INSERT INTO edit_steps
                     (id, sequence_id, position, file, search, replace, verification_cmd, status, generated)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        step.id,
                        step.sequence_id,
                        step.position as i64,
                        step.file.to_string_lossy().as_ref(),
                        step.search,
                        step.replace,
                        step.verification_cmd,
                        step.status.as_str(),
                        step.generated as i64,
                    ],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }

        Ok(())
    }

    pub(crate) fn get_sequence(&self, id: &str) -> Result<Option<EditSequence>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, task_id, goal, current_idx, status, snapshot_ref
                 FROM edit_sequences WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let (seq_id, task_id, goal, current_idx, status_str, snapshot_ref) = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let steps = self.load_steps(&seq_id)?;

        Ok(Some(EditSequence {
            id: seq_id,
            task_id,
            goal,
            steps,
            current_idx: current_idx as usize,
            status: SequenceStatus::from_str(&status_str),
            snapshot_ref,
        }))
    }

    pub(crate) fn get_current_step(&self, sequence_id: &str) -> Result<Option<EditStep>> {
        let row = self
            .conn
            .query_row(
                "SELECT es.id, es.sequence_id, es.position, es.file, es.search, es.replace,
                        es.verification_cmd, es.status, es.generated
                 FROM edit_steps es
                 JOIN edit_sequences eq ON eq.id = es.sequence_id
                 WHERE es.sequence_id = ?1 AND es.position = eq.current_idx",
                params![sequence_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        Ok(row.map(
            |(
                id,
                sequence_id,
                position,
                file,
                search,
                replace,
                verification_cmd,
                status_str,
                generated,
            )| {
                EditStep {
                    id,
                    sequence_id,
                    position: position as usize,
                    file: PathBuf::from(file),
                    search,
                    replace,
                    verification_cmd,
                    status: StepStatus::from_str(&status_str),
                    generated: generated != 0,
                }
            },
        ))
    }

    pub(crate) fn advance(&self, sequence_id: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE edit_sequences SET current_idx = current_idx + 1 WHERE id = ?1",
                params![sequence_id],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Shift all steps at positions >= after_position+1 up by N, then insert steps at
    /// consecutive positions starting at after_position+1. The `position` field in each
    /// provided step is ignored — actual positions are assigned here.
    pub(crate) fn insert_step_after(
        &self,
        sequence_id: &str,
        after_position: usize,
        steps: Vec<EditStep>,
    ) -> Result<()> {
        if steps.is_empty() {
            return Ok(());
        }
        let n = steps.len() as i64;
        let shift_from = (after_position + 1) as i64;

        self.conn
            .execute("BEGIN IMMEDIATE", [])
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let result: Result<()> = (|| {
            self.conn
                .execute(
                    "UPDATE edit_steps SET position = position + ?1 \
                     WHERE sequence_id = ?2 AND position >= ?3",
                    params![n, sequence_id, shift_from],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;

            for (i, step) in steps.iter().enumerate() {
                let pos = (after_position + 1 + i) as i64;
                self.conn
                    .execute(
                        "INSERT INTO edit_steps \
                         (id, sequence_id, position, file, search, replace, \
                          verification_cmd, status, generated) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            step.id,
                            step.sequence_id,
                            pos,
                            step.file.to_string_lossy().as_ref(),
                            step.search,
                            step.replace,
                            step.verification_cmd,
                            step.status.as_str(),
                            step.generated as i64,
                        ],
                    )
                    .map_err(|e| AppError::Storage(e.to_string()))?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => self
                .conn
                .execute("COMMIT", [])
                .map(|_| ())
                .map_err(|e| AppError::Storage(e.to_string())),
            Err(e) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(e)
            }
        }
    }

    pub(crate) fn count_steps(&self, sequence_id: &str) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM edit_steps WHERE sequence_id = ?1",
                params![sequence_id],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(count as usize)
    }

    pub(crate) fn update_step_status(&self, step_id: &str, status: StepStatus) -> Result<()> {
        self.conn
            .execute(
                "UPDATE edit_steps SET status = ?1 WHERE id = ?2",
                params![status.as_str(), step_id],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn update_sequence_status(
        &self,
        sequence_id: &str,
        status: SequenceStatus,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE edit_sequences SET status = ?1 WHERE id = ?2",
                params![status.as_str(), sequence_id],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn get_latest_sequence(&self) -> Result<Option<EditSequence>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, task_id, goal, current_idx, status, snapshot_ref
                 FROM edit_sequences ORDER BY rowid DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let (seq_id, task_id, goal, current_idx, status_str, snapshot_ref) = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let steps = self.load_steps(&seq_id)?;

        Ok(Some(EditSequence {
            id: seq_id,
            task_id,
            goal,
            steps,
            current_idx: current_idx as usize,
            status: SequenceStatus::from_str(&status_str),
            snapshot_ref,
        }))
    }

    fn load_steps(&self, sequence_id: &str) -> Result<Vec<EditStep>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, sequence_id, position, file, search, replace, verification_cmd, status, generated
                 FROM edit_steps WHERE sequence_id = ?1 ORDER BY position ASC",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let steps = stmt
            .query_map(params![sequence_id], |row| {
                Ok(EditStep {
                    id: row.get(0)?,
                    sequence_id: row.get(1)?,
                    position: row.get::<_, i64>(2)? as usize,
                    file: PathBuf::from(row.get::<_, String>(3)?),
                    search: row.get(4)?,
                    replace: row.get(5)?,
                    verification_cmd: row.get(6)?,
                    status: StepStatus::from_str(&row.get::<_, String>(7)?),
                    generated: row.get::<_, i64>(8)? != 0,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        Ok(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn make_store() -> EditSequenceStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS edit_sequences (
                id TEXT PRIMARY KEY, task_id TEXT, goal TEXT NOT NULL,
                current_idx INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'pending', snapshot_ref TEXT
             );
             CREATE TABLE IF NOT EXISTS edit_steps (
                id TEXT PRIMARY KEY, sequence_id TEXT NOT NULL,
                position INTEGER NOT NULL, file TEXT NOT NULL,
                search TEXT NOT NULL, replace TEXT NOT NULL,
                verification_cmd TEXT, status TEXT NOT NULL DEFAULT 'pending',
                generated INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();
        EditSequenceStore::new(conn)
    }

    fn make_sequence(id: &str, goal: &str) -> EditSequence {
        EditSequence {
            id: id.to_string(),
            task_id: None,
            goal: goal.to_string(),
            steps: vec![EditStep {
                id: format!("{id}-step0"),
                sequence_id: id.to_string(),
                position: 0,
                file: std::path::PathBuf::from("src/lib.rs"),
                search: "fn old".to_string(),
                replace: "fn new".to_string(),
                verification_cmd: None,
                status: StepStatus::Pending,
                generated: false,
            }],
            current_idx: 0,
            status: SequenceStatus::Planning,
            snapshot_ref: None,
        }
    }

    #[test]
    fn update_sequence_status_changes_status() {
        let store = make_store();
        let seq = make_sequence("seq1", "refactor auth");
        store.create_sequence(&seq).unwrap();

        store
            .update_sequence_status("seq1", SequenceStatus::Approved)
            .unwrap();

        let loaded = store.get_sequence("seq1").unwrap().unwrap();
        assert_eq!(loaded.status, SequenceStatus::Approved);
    }

    #[test]
    fn get_latest_sequence_returns_most_recent() {
        let store = make_store();
        store
            .create_sequence(&make_sequence("seq-a", "first goal"))
            .unwrap();
        store
            .create_sequence(&make_sequence("seq-b", "second goal"))
            .unwrap();

        let latest = store.get_latest_sequence().unwrap().unwrap();
        assert_eq!(latest.id, "seq-b");
        assert_eq!(latest.goal, "second goal");
    }

    #[test]
    fn get_latest_sequence_returns_none_when_empty() {
        let store = make_store();
        assert!(store.get_latest_sequence().unwrap().is_none());
    }

    #[test]
    fn get_latest_sequence_loads_steps() {
        let store = make_store();
        store
            .create_sequence(&make_sequence("seq1", "goal"))
            .unwrap();

        let seq = store.get_latest_sequence().unwrap().unwrap();
        assert_eq!(seq.steps.len(), 1);
        assert_eq!(seq.steps[0].search, "fn old");
    }

    #[test]
    fn insert_step_after_shifts_and_inserts_at_correct_position() {
        let store = make_store();

        // 3-step sequence; steps 0 and 1 are done, current_idx = 2.
        let seq = EditSequence {
            id: "seq_ins".to_string(),
            task_id: None,
            goal: "test insert".to_string(),
            steps: vec![
                EditStep {
                    id: "s0".to_string(),
                    sequence_id: "seq_ins".to_string(),
                    position: 0,
                    file: std::path::PathBuf::from("src/a.rs"),
                    search: "fn a".to_string(),
                    replace: "fn a_new".to_string(),
                    verification_cmd: None,
                    status: StepStatus::Verified,
                    generated: false,
                },
                EditStep {
                    id: "s1".to_string(),
                    sequence_id: "seq_ins".to_string(),
                    position: 1,
                    file: std::path::PathBuf::from("src/b.rs"),
                    search: "fn b".to_string(),
                    replace: "fn b_new".to_string(),
                    verification_cmd: None,
                    status: StepStatus::Verified,
                    generated: false,
                },
                EditStep {
                    id: "s2".to_string(),
                    sequence_id: "seq_ins".to_string(),
                    position: 2,
                    file: std::path::PathBuf::from("src/c.rs"),
                    search: "fn c".to_string(),
                    replace: "fn c_new".to_string(),
                    verification_cmd: None,
                    status: StepStatus::Pending,
                    generated: false,
                },
            ],
            current_idx: 2,
            status: SequenceStatus::Approved,
            snapshot_ref: None,
        };
        store.create_sequence(&seq).unwrap();

        // Insert a generated placeholder after position 1 (placeholder position = 0).
        let new_step = EditStep {
            id: "s_new".to_string(),
            sequence_id: "seq_ins".to_string(),
            position: 0, // arbitrary placeholder — insert_step_after must overwrite
            file: std::path::PathBuf::from("src/d.rs"),
            search: String::new(),
            replace: String::new(),
            verification_cmd: None,
            status: StepStatus::Pending,
            generated: true,
        };
        store
            .insert_step_after("seq_ins", 1, vec![new_step])
            .unwrap();

        let loaded = store.get_sequence("seq_ins").unwrap().unwrap();
        assert_eq!(loaded.steps.len(), 4);

        // New step must be at position 2 — NOT the placeholder value 0.
        let new_s = loaded.steps.iter().find(|s| s.id == "s_new").unwrap();
        assert_eq!(
            new_s.position, 2,
            "new step must be at position 2, not placeholder 0"
        );
        assert!(new_s.search.is_empty());
        assert!(new_s.generated);

        // Old step-2 must have shifted to position 3.
        let old_s2 = loaded.steps.iter().find(|s| s.id == "s2").unwrap();
        assert_eq!(old_s2.position, 3, "old step-2 must shift to position 3");

        // get_current_step with current_idx = 2 returns the newly inserted step.
        let current = store.get_current_step("seq_ins").unwrap().unwrap();
        assert_eq!(
            current.id, "s_new",
            "current step (idx=2) must be the newly inserted step"
        );
        assert_eq!(current.position, 2);
    }
}
