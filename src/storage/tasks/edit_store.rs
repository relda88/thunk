use std::path::PathBuf;

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
    fn as_str(&self) -> &'static str {
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
                     (id, sequence_id, position, file, search, replace, verification_cmd, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        step.id,
                        step.sequence_id,
                        step.position as i64,
                        step.file.to_string_lossy().as_ref(),
                        step.search,
                        step.replace,
                        step.verification_cmd,
                        step.status.as_str(),
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
                        es.verification_cmd, es.status
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
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        Ok(row.map(
            |(id, sequence_id, position, file, search, replace, verification_cmd, status_str)| {
                EditStep {
                    id,
                    sequence_id,
                    position: position as usize,
                    file: PathBuf::from(file),
                    search,
                    replace,
                    verification_cmd,
                    status: StepStatus::from_str(&status_str),
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

    pub(crate) fn update_step_status(&self, step_id: &str, status: StepStatus) -> Result<()> {
        self.conn
            .execute(
                "UPDATE edit_steps SET status = ?1 WHERE id = ?2",
                params![status.as_str(), step_id],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    fn load_steps(&self, sequence_id: &str) -> Result<Vec<EditStep>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, sequence_id, position, file, search, replace, verification_cmd, status
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
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| AppError::Storage(e.to_string()))?;

        Ok(steps)
    }
}
