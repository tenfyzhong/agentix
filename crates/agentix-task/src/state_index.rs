//! Borrowed lookups for one state-difference transaction.
use std::collections::HashMap;

use crate::{InboxEntry, Job, Lease, Plan, Project, Snapshot, Task};

pub(crate) struct StateIndex<'a> {
    pub projects: HashMap<&'a str, &'a Project>,
    pub jobs: HashMap<&'a str, &'a Job>,
    pub tasks: HashMap<&'a str, &'a Task>,
    pub plans: HashMap<&'a str, &'a Plan>,
    pub leases: HashMap<&'a str, &'a Lease>,
    pub inboxes: HashMap<&'a str, &'a InboxEntry>,
}

fn by_id<T>(items: &[T], id: impl Fn(&T) -> &str) -> HashMap<&str, &T> {
    #[cfg(test)]
    crate::store::persist_tests::VISITS.with(|count| count.set(count.get() + items.len()));
    let mut result = HashMap::with_capacity(items.len());
    for item in items {
        result.entry(id(item)).or_insert(item);
    }
    result
}

impl<'a> StateIndex<'a> {
    pub fn new(state: &'a Snapshot) -> Self {
        Self {
            projects: by_id(&state.projects, |v| &v.id),
            jobs: by_id(&state.jobs, |v| &v.id),
            tasks: by_id(&state.tasks, |v| &v.id),
            plans: by_id(&state.plans, |v| &v.id),
            leases: by_id(&state.leases, |v| &v.task_id),
            inboxes: by_id(&state.inboxes, |v| &v.id),
        }
    }
}

pub(crate) fn recent_session_tasks(state: &Snapshot) -> HashMap<&str, &Task> {
    let mut result: HashMap<&str, &Task> = HashMap::new();
    for task in &state.tasks {
        #[cfg(test)]
        crate::store::persist_tests::VISITS.with(|count| count.set(count.get() + 1));
        if task.last_session.is_some() {
            let current = result.entry(task.job_id.as_str()).or_insert(task);
            // Iterator::max_by_key selects the last item when timestamps tie.
            if task.updated_at >= current.updated_at {
                *current = task;
            }
        }
    }
    result
}
