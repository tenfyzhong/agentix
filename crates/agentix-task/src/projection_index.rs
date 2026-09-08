//! Borrowed relationships prepared once for a document publication snapshot.
use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::{Job, Plan, Project, Snapshot, Task};

pub(super) struct ProjectionIndex<'a> {
    projects: HashMap<&'a str, &'a Project>,
    jobs: HashMap<&'a str, &'a Job>,
    tasks: HashMap<&'a str, &'a Task>,
    pub plans: HashMap<&'a str, &'a Plan>,
    pub jobs_by_project: HashMap<&'a str, Vec<&'a Job>>,
    pub tasks_by_job: HashMap<&'a str, Vec<&'a Task>>,
}

impl<'a> ProjectionIndex<'a> {
    pub fn new(state: &'a Snapshot) -> Self {
        let mut index = Self {
            projects: state.projects.iter().map(|p| (p.id.as_str(), p)).collect(),
            jobs: state.jobs.iter().map(|j| (j.id.as_str(), j)).collect(),
            tasks: state.tasks.iter().map(|t| (t.id.as_str(), t)).collect(),
            plans: state.plans.iter().map(|p| (p.id.as_str(), p)).collect(),
            jobs_by_project: HashMap::new(),
            tasks_by_job: HashMap::new(),
        };
        for job in &state.jobs {
            index
                .jobs_by_project
                .entry(&job.project_id)
                .or_default()
                .push(job);
        }
        for task in &state.tasks {
            index
                .tasks_by_job
                .entry(&task.job_id)
                .or_default()
                .push(task);
        }
        index
    }

    pub fn project(&self, id: &str) -> Result<&'a Project> {
        self.projects
            .get(id)
            .copied()
            .with_context(|| format!("not_found: {id}"))
    }

    pub fn job(&self, id: &str) -> Result<&'a Job> {
        self.jobs
            .get(id)
            .copied()
            .with_context(|| format!("not_found: {id}"))
    }

    pub fn task(&self, id: &str) -> Result<&'a Task> {
        self.tasks
            .get(id)
            .copied()
            .with_context(|| format!("not_found: {id}"))
    }

    pub fn task_path(&self, task: &Task) -> Result<String> {
        crate::naming::task_path_in(self.project(&task.project_id)?, task)
    }
}
