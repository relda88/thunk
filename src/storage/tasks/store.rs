use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use super::types::{PlanRecord, PlanStatus, TaskRecord, TaskStatus};
use crate::core::error::{AppError, Result};

pub(crate) struct TaskStore {
    conn: Connection,
}

impl TaskStore {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(Self { conn })
    }

    pub(crate) fn create_plan(
        &self,
        session_id: &str,
        project_root: &str,
        goal: &str,
    ) -> Result<String> {
        let id = generate_id();
        let now = now_str();
        self.conn
            .execute(
                "INSERT INTO plans (id, session_id, project_root, goal, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'draft', ?5, ?5)",
                params![id, session_id, project_root, goal, now],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(id)
    }

    pub(crate) fn add_task(
        &self,
        plan_id: &str,
        session_id: &str,
        project_root: &str,
        step_number: usize,
        title: &str,
        description: &str,
    ) -> Result<String> {
        let id = generate_id();
        let now = now_str();
        self.conn
            .execute(
                "INSERT INTO plan_tasks
                 (id, plan_id, session_id, project_root, step_number, title, description,
                  status, result_summary, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', '', ?8, ?8)",
                params![
                    id,
                    plan_id,
                    session_id,
                    project_root,
                    step_number as i64,
                    title,
                    description,
                    now
                ],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(id)
    }

    pub(crate) fn get_active_plan(
        &self,
        session_id: &str,
        project_root: &str,
    ) -> Result<Option<PlanRecord>> {
        self.conn
            .query_row(
                "SELECT id, session_id, project_root, goal, status, created_at, updated_at
                 FROM plans
                 WHERE session_id = ?1 AND project_root = ?2 AND status = 'active'
                 LIMIT 1",
                params![session_id, project_root],
                |row| {
                    Ok(PlanRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        project_root: row.get(2)?,
                        goal: row.get(3)?,
                        status: PlanStatus::Active,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    pub(crate) fn list_tasks(&self, plan_id: &str) -> Result<Vec<TaskRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, plan_id, session_id, project_root, step_number, title, description,
                        status, result_summary, created_at, updated_at
                 FROM plan_tasks
                 WHERE plan_id = ?1
                 ORDER BY step_number ASC",
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![plan_id], |row| {
                let status_str: String = row.get(7)?;
                Ok(TaskRecord {
                    id: row.get(0)?,
                    plan_id: row.get(1)?,
                    session_id: row.get(2)?,
                    project_root: row.get(3)?,
                    step_number: row.get::<_, i64>(4)? as usize,
                    title: row.get(5)?,
                    description: row.get(6)?,
                    status: TaskStatus::from_str(&status_str),
                    result_summary: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub(crate) fn update_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
        result_summary: Option<&str>,
    ) -> Result<()> {
        let now = now_str();
        if let Some(summary) = result_summary {
            self.conn
                .execute(
                    "UPDATE plan_tasks SET status = ?2, result_summary = ?3, updated_at = ?4
                     WHERE id = ?1",
                    params![task_id, status.as_str(), summary, now],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;
        } else {
            self.conn
                .execute(
                    "UPDATE plan_tasks SET status = ?2, updated_at = ?3 WHERE id = ?1",
                    params![task_id, status.as_str(), now],
                )
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn update_plan_status(&self, plan_id: &str, status: PlanStatus) -> Result<()> {
        let now = now_str();
        self.conn
            .execute(
                "UPDATE plans SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![plan_id, status.as_str(), now],
            )
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }
}

fn now_str() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn generate_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = nanos ^ (std::process::id() as u128);
    format!("{:016x}", unique & 0xFFFF_FFFF_FFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::storage::session::schema;

    fn in_memory() -> TaskStore {
        let conn = Connection::open_in_memory().unwrap();
        schema::initialize(&conn).unwrap();
        TaskStore { conn }
    }

    #[test]
    fn create_plan_returns_id() {
        let store = in_memory();
        let id = store
            .create_plan("session1", "/proj", "build a thing")
            .unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn add_task_returns_id() {
        let store = in_memory();
        let plan_id = store.create_plan("session1", "/proj", "goal").unwrap();
        let task_id = store
            .add_task(&plan_id, "session1", "/proj", 1, "first task", "do it")
            .unwrap();
        assert!(!task_id.is_empty());
    }

    #[test]
    fn get_active_plan_returns_none_when_empty() {
        let store = in_memory();
        let result = store.get_active_plan("session1", "/proj").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_active_plan_returns_plan_after_activate() {
        let store = in_memory();
        let id = store.create_plan("session1", "/proj", "my goal").unwrap();
        store.update_plan_status(&id, PlanStatus::Active).unwrap();
        let plan = store.get_active_plan("session1", "/proj").unwrap().unwrap();
        assert_eq!(plan.id, id);
        assert_eq!(plan.goal, "my goal");
        assert_eq!(plan.status, PlanStatus::Active);
    }

    #[test]
    fn list_tasks_ordered_by_step() {
        let store = in_memory();
        let plan_id = store.create_plan("session1", "/proj", "goal").unwrap();
        store
            .add_task(&plan_id, "session1", "/proj", 3, "third", "")
            .unwrap();
        store
            .add_task(&plan_id, "session1", "/proj", 1, "first", "")
            .unwrap();
        store
            .add_task(&plan_id, "session1", "/proj", 2, "second", "")
            .unwrap();

        let tasks = store.list_tasks(&plan_id).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].step_number, 1);
        assert_eq!(tasks[1].step_number, 2);
        assert_eq!(tasks[2].step_number, 3);
        assert_eq!(tasks[0].title, "first");
        assert_eq!(tasks[2].title, "third");
    }

    #[test]
    fn update_task_status_persists() {
        let store = in_memory();
        let plan_id = store.create_plan("session1", "/proj", "goal").unwrap();
        let task_id = store
            .add_task(&plan_id, "session1", "/proj", 1, "task", "")
            .unwrap();

        store
            .update_task_status(&task_id, TaskStatus::InProgress, Some("started"))
            .unwrap();

        let tasks = store.list_tasks(&plan_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::InProgress);
        assert_eq!(tasks[0].result_summary, "started");
    }

    #[test]
    fn cross_project_isolation() {
        let store = in_memory();
        let id = store
            .create_plan("session1", "/project-a", "plan for a")
            .unwrap();
        store.update_plan_status(&id, PlanStatus::Active).unwrap();

        let result = store.get_active_plan("session1", "/project-b").unwrap();
        assert!(
            result.is_none(),
            "must not return plans from a different project"
        );
    }
}
