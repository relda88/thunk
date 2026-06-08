use super::super::super::protocol::plan_parser::parse_plan;
use super::super::super::trace::trace_runtime_decision;
use super::super::super::types::{Activity, RuntimeEvent};
use super::command_handlers::PendingPlanDraft;
use super::Runtime;
use crate::llm::backend::{BackendEvent, GenerateRequest, Message};
use crate::storage::tasks::{PlanStatus, TaskStatus};

impl Runtime {
    pub(super) fn handle_plan_create(
        &mut self,
        goal: String,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "plan: cannot create plan while a tool approval is pending".to_string(),
            ));
            return;
        }

        let project_root = self.project_root.path().to_string_lossy().to_string();
        if let Some(store) = &self.task_store {
            match store.get_active_plan(&self.session_id, &project_root) {
                Ok(Some(existing)) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "plan: '{}' is already active — use /plan abandon first",
                        existing.goal
                    )));
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "plan: storage error: {e}"
                    )));
                    return;
                }
            }
        }

        on_event(RuntimeEvent::SystemMessage(
            "plan: generating...".to_string(),
        ));

        let mut steps_result: Result<Vec<_>, String> = Err("no response".to_string());
        for attempt in 0..2 {
            match self.generate_plan_steps(&goal) {
                Some(text) => match parse_plan(&text) {
                    Ok(steps) => {
                        steps_result = Ok(steps);
                        break;
                    }
                    Err(e) => {
                        trace_runtime_decision(
                            on_event,
                            "plan_parse_failed",
                            &[("attempt", attempt.to_string())],
                        );
                        if attempt == 0 {
                            on_event(RuntimeEvent::SystemMessage(format!(
                                "plan: retrying... ({e})"
                            )));
                        } else {
                            steps_result = Err(e);
                        }
                    }
                },
                None => {
                    steps_result = Err("backend did not respond".to_string());
                    break;
                }
            }
        }

        match steps_result {
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "plan: could not parse model response — {e} — try again or simplify the goal"
                )));
            }
            Ok(steps) => {
                let step_pairs: Vec<(String, String)> = steps
                    .iter()
                    .map(|s| (s.title.clone(), s.description.clone()))
                    .collect();
                trace_runtime_decision(
                    on_event,
                    "plan_draft_created",
                    &[("steps", steps.len().to_string())],
                );
                self.pending_plan = Some(PendingPlanDraft {
                    goal: goal.clone(),
                    steps,
                });
                on_event(RuntimeEvent::PlanApprovalRequired {
                    goal,
                    steps: step_pairs,
                });
            }
        }
    }

    pub(super) fn handle_plan_approve(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let draft = match self.pending_plan.take() {
            Some(d) => d,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "plan: no pending plan to approve".to_string(),
                ));
                return;
            }
        };

        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "plan: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan_id = match store.create_plan(&self.session_id, &project_root, &draft.goal) {
            Ok(id) => id,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "plan: failed to save plan: {e}"
                )));
                return;
            }
        };

        for (i, step) in draft.steps.iter().enumerate() {
            if let Err(e) = store.add_task(
                &plan_id,
                &self.session_id,
                &project_root,
                i + 1,
                &step.title,
                &step.description,
            ) {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "plan: failed to save step {}: {e}",
                    i + 1
                )));
                return;
            }
        }

        if let Err(e) = store.update_plan_status(&plan_id, PlanStatus::Active) {
            on_event(RuntimeEvent::SystemMessage(format!(
                "plan: failed to activate plan: {e}"
            )));
            return;
        }

        trace_runtime_decision(on_event, "plan_approved", &[("plan_id", plan_id.clone())]);
        on_event(RuntimeEvent::PlanApprovalCleared);
        on_event(RuntimeEvent::SystemMessage(
            "plan: approved and saved — use /plan status to view steps".to_string(),
        ));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    pub(super) fn handle_plan_abandon(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        self.pending_plan = None;

        let project_root = self.project_root.path().to_string_lossy().to_string();
        if let Some(store) = &self.task_store {
            if let Ok(Some(plan)) = store.get_active_plan(&self.session_id, &project_root) {
                let _ = store.update_plan_status(&plan.id, PlanStatus::Abandoned);
                trace_runtime_decision(on_event, "plan_abandoned", &[("plan_id", plan.id.clone())]);
            }
        }

        on_event(RuntimeEvent::PlanApprovalCleared);
        on_event(RuntimeEvent::SystemMessage("plan: abandoned".to_string()));
        on_event(RuntimeEvent::ActivityChanged(Activity::Idle));
    }

    pub(super) fn handle_plan_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if let Some(ref draft) = self.pending_plan {
            let mut msg = format!("plan (pending approval): {}\n", draft.goal);
            for (i, step) in draft.steps.iter().enumerate() {
                msg.push_str(&format!(
                    "  {}. {}: {}\n",
                    i + 1,
                    step.title,
                    step.description
                ));
            }
            msg.push_str("  Use /plan approve or /plan abandon");
            on_event(RuntimeEvent::SystemMessage(msg));
            return;
        }

        let project_root = self.project_root.path().to_string_lossy().to_string();
        if let Some(store) = &self.task_store {
            match store.get_active_plan(&self.session_id, &project_root) {
                Ok(Some(plan)) => {
                    let mut msg = format!("plan: {}\n", plan.goal);
                    match store.list_tasks(&plan.id) {
                        Ok(tasks) => {
                            for task in &tasks {
                                msg.push_str(&format!(
                                    "  {}. [{}] {}: {}\n",
                                    task.step_number,
                                    task.status.as_str(),
                                    task.title,
                                    task.description
                                ));
                            }
                        }
                        Err(e) => msg.push_str(&format!("  (error loading tasks: {e})")),
                    }
                    on_event(RuntimeEvent::SystemMessage(msg));
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    on_event(RuntimeEvent::SystemMessage(format!(
                        "plan: storage error: {e}"
                    )));
                    return;
                }
            }
        }

        on_event(RuntimeEvent::SystemMessage(
            "plan: no active plan".to_string(),
        ));
    }

    /// NOTE: This function calls self.backend.generate() directly, bypassing
    /// generation.rs and the turn loop. Known violation — deferred to Phase 40.
    pub(super) fn generate_plan_steps(&mut self, goal: &str) -> Option<String> {
        let prompt = format!(
            "You are generating a structured implementation plan.\n\
             Respond with ONLY a numbered list — no preamble, no prose,\n\
             no markdown headers, no trailing commentary.\n\n\
             Format (one step per line):\n\
             1. Title: Brief description of this step\n\
             2. Title: Brief description of this step\n\n\
             Goal: {goal}\n\n\
             Requirements:\n\
             - At least 2 steps, at most 10 steps\n\
             - Each line must match exactly: N. Title: Description\n\
             - No blank lines between steps\n\
             - No text before or after the numbered list"
        );
        let mut messages = self.conversation.pruned_snapshot();
        messages.push(Message::user(prompt));
        let request = GenerateRequest::new(messages);
        let mut result = String::new();
        if self
            .backend
            .generate(request, &mut |event| {
                if let BackendEvent::TextDelta(chunk) = event {
                    result.push_str(&chunk);
                }
            })
            .is_err()
        {
            return None;
        }
        let text = result.trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub(super) fn handle_task_execute(
        &mut self,
        step: usize,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if self.pending_action.is_some() {
            on_event(RuntimeEvent::SystemMessage(
                "task: cannot execute while a tool approval is pending".to_string(),
            ));
            return;
        }

        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan = match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(p)) => p,
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan — use /plan first".to_string(),
                ));
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let tasks = match store.list_tasks(&plan.id) {
            Ok(t) => t,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let task = match tasks.iter().find(|t| t.step_number == step) {
            Some(t) => t.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: step {step} not found in active plan"
                )));
                return;
            }
        };

        let total = tasks.len();

        if task.status == TaskStatus::Completed {
            on_event(RuntimeEvent::SystemMessage(format!(
                "task: step {step} is already completed — executing anyway"
            )));
        }

        if let Some(store) = &self.task_store {
            let _ = store.update_task_status(&task.id, TaskStatus::InProgress, None);
        }
        trace_runtime_decision(
            on_event,
            "task_status_changed",
            &[
                ("step", step.to_string()),
                ("old", format!("{:?}", task.status)),
                ("new", "InProgress".into()),
            ],
        );

        let augmented_prompt = format!(
            "[runtime:task] Step {step} of {total}: \"{title}\"\n\
             Description: {description}\n\
             Plan goal: {goal}\n\n\
             Work on this step now.",
            title = task.title,
            description = task.description,
            goal = plan.goal,
        );

        self.conversation.push_user(augmented_prompt);
        on_event(RuntimeEvent::ActivityChanged(Activity::Processing));
        self.run_turns(0, on_event);
    }

    pub(super) fn handle_task_complete(
        &mut self,
        step: usize,
        summary: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan = match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(p)) => p,
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan".to_string(),
                ));
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let tasks = match store.list_tasks(&plan.id) {
            Ok(t) => t,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let task = match tasks.iter().find(|t| t.step_number == step) {
            Some(t) => t.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: step {step} not found in active plan"
                )));
                return;
            }
        };

        if let Err(e) =
            store.update_task_status(&task.id, TaskStatus::Completed, summary.as_deref())
        {
            on_event(RuntimeEvent::SystemMessage(format!(
                "task: storage error: {e}"
            )));
            return;
        }

        trace_runtime_decision(
            on_event,
            "task_status_changed",
            &[
                ("step", step.to_string()),
                ("old", format!("{:?}", task.status)),
                ("new", "Completed".into()),
            ],
        );
        on_event(RuntimeEvent::SystemMessage(format!(
            "task {step}: completed"
        )));

        let all_done = tasks
            .iter()
            .filter(|t| t.step_number != step)
            .all(|t| t.status == TaskStatus::Completed);
        if all_done {
            if let Some(store) = &self.task_store {
                let _ = store.update_plan_status(&plan.id, PlanStatus::Completed);
            }
            trace_runtime_decision(on_event, "plan_completed", &[("plan_id", plan.id.clone())]);
            on_event(RuntimeEvent::SystemMessage(
                "plan: all steps completed".to_string(),
            ));
        }
    }

    pub(super) fn handle_task_block(
        &mut self,
        step: usize,
        reason: Option<String>,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        let plan = match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(p)) => p,
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan".to_string(),
                ));
                return;
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let tasks = match store.list_tasks(&plan.id) {
            Ok(t) => t,
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
                return;
            }
        };

        let task = match tasks.iter().find(|t| t.step_number == step) {
            Some(t) => t.clone(),
            None => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: step {step} not found in active plan"
                )));
                return;
            }
        };

        if let Err(e) = store.update_task_status(&task.id, TaskStatus::Blocked, reason.as_deref()) {
            on_event(RuntimeEvent::SystemMessage(format!(
                "task: storage error: {e}"
            )));
            return;
        }

        trace_runtime_decision(
            on_event,
            "task_status_changed",
            &[
                ("step", step.to_string()),
                ("old", format!("{:?}", task.status)),
                ("new", "Blocked".into()),
            ],
        );
        on_event(RuntimeEvent::SystemMessage(format!("task {step}: blocked")));
    }

    pub(super) fn handle_task_status(&mut self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        let store = match &self.task_store {
            Some(s) => s,
            None => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no storage configured".to_string(),
                ));
                return;
            }
        };

        let project_root = self.project_root.path().to_string_lossy().to_string();
        match store.get_active_plan(&self.session_id, &project_root) {
            Ok(Some(plan)) => {
                let mut msg = format!("plan: {}\n", plan.goal);
                match store.list_tasks(&plan.id) {
                    Ok(tasks) => {
                        for task in &tasks {
                            let badge = match task.status {
                                TaskStatus::Pending => "[pending]",
                                TaskStatus::InProgress => "[in_progress]",
                                TaskStatus::Completed => "[completed]",
                                TaskStatus::Blocked => "[blocked]",
                            };
                            msg.push_str(&format!(
                                "  {}. {} {}: {}\n",
                                task.step_number, badge, task.title, task.description
                            ));
                        }
                    }
                    Err(e) => msg.push_str(&format!("  (error loading tasks: {e})")),
                }
                on_event(RuntimeEvent::SystemMessage(msg));
            }
            Ok(None) => {
                on_event(RuntimeEvent::SystemMessage(
                    "task: no active plan".to_string(),
                ));
            }
            Err(e) => {
                on_event(RuntimeEvent::SystemMessage(format!(
                    "task: storage error: {e}"
                )));
            }
        }
    }
}
