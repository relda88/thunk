#[derive(Debug, Clone, PartialEq)]
pub enum PlanStatus {
    Draft,
    Active,
    Completed,
    Abandoned,
}

impl PlanStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "completed" => Self::Completed,
            "abandoned" => Self::Abandoned,
            _ => Self::Draft,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "blocked" => Self::Blocked,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanRecord {
    pub id: String,
    pub session_id: String,
    pub project_root: String,
    pub goal: String,
    pub status: PlanStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: String,
    pub plan_id: String,
    pub session_id: String,
    pub project_root: String,
    pub step_number: usize,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub result_summary: String,
    pub created_at: String,
    pub updated_at: String,
}
