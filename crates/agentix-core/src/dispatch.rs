//! Bounded resource-aware admission shared by application runtimes.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::Hash,
    time::{Duration, Instant},
};

/// Resources that must remain ordered for one operation.
#[derive(Debug, Clone)]
pub enum DispatchScope<K> {
    Keys(Vec<K>),
    /// Readers share a resource; any exclusive operation fences all readers.
    Access {
        shared: Vec<K>,
        exclusive: Vec<K>,
    },
    /// A lifecycle transition that must observe all preceding work and finish
    /// before any subsequent work starts.
    Global,
}

impl<K> DispatchScope<K> {
    fn access(&self) -> Option<(&[K], &[K])> {
        match self {
            Self::Global => None,
            Self::Keys(keys) => Some((&[], keys)),
            Self::Access { shared, exclusive } => Some((shared, exclusive)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchId(u64);

pub struct Dispatched<T> {
    pub id: DispatchId,
    pub work: T,
    pub queued_for: Duration,
}

/// Bounded cumulative counters and current gauges; contains no message content.
#[derive(Clone, Debug, Default)]
pub struct DispatchStatistics {
    pub pending: usize,
    pub active: usize,
    pub admitted: u64,
    pub rejected: u64,
    pub started: u64,
    pub completed: u64,
    pub wait_total: Duration,
    pub wait_max: Duration,
    pub execution_total: Duration,
    pub execution_max: Duration,
    pub oldest_pending: Duration,
}

impl DispatchStatistics {
    /// Periodic structured output, with low-cardinality runtime labels.
    pub fn record(&self, runtime: &'static str) {
        tracing::info!(
            target: "agentix::telemetry",
            runtime,
            pending = self.pending,
            active = self.active,
            admitted = self.admitted,
            rejected = self.rejected,
            started = self.started,
            retired = self.completed,
            wait_total_ms = self.wait_total.as_secs_f64() * 1000.0,
            wait_max_ms = self.wait_max.as_secs_f64() * 1000.0,
            execution_total_ms = self.execution_total.as_secs_f64() * 1000.0,
            execution_max_ms = self.execution_max.as_secs_f64() * 1000.0,
            oldest_pending_ms = self.oldest_pending.as_secs_f64() * 1000.0,
            "dispatch statistics"
        );
    }
}

/// Owns admission, ordering and bounds, but never spawns detached tasks.
///
/// A runtime must finish each dispatched ID when its worker completes, panics or
/// is cancelled. Pending work has no side effects and is dropped with the queue.
/// Scope resolution is synchronous and must use a current local routing snapshot;
/// it must not perform network I/O or mutate application state.
pub struct DispatchQueue<K, T> {
    capacity: usize,
    concurrency: usize,
    next_id: u64,
    pending: VecDeque<(T, Instant)>,
    active: HashMap<DispatchId, DispatchScope<K>>,
    started_at: HashMap<DispatchId, Instant>,
    statistics: DispatchStatistics,
}

impl<K: Clone + Eq + Hash, T> DispatchQueue<K, T> {
    #[must_use]
    pub fn new(capacity: usize, concurrency: usize) -> Self {
        assert!(
            capacity > 0 && concurrency > 0,
            "dispatch limits must be positive"
        );
        Self {
            capacity,
            concurrency,
            next_id: 0,
            pending: VecDeque::new(),
            active: HashMap::new(),
            started_at: HashMap::new(),
            statistics: DispatchStatistics::default(),
        }
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.pending.len() + self.active.len() >= self.capacity
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.active.is_empty()
    }

    /// Whether resolving current routes can dispatch any work. Resource
    /// conflicts still require a fresh snapshot, including after completions.
    #[must_use]
    pub fn needs_routing(&self) -> bool {
        !self.pending.is_empty() && self.active.len() < self.concurrency
    }

    #[must_use]
    pub fn statistics(&self) -> DispatchStatistics {
        DispatchStatistics {
            pending: self.pending.len(),
            active: self.active.len(),
            oldest_pending: self
                .pending
                .front()
                .map_or(Duration::ZERO, |(_, at)| at.elapsed()),
            ..self.statistics.clone()
        }
    }

    /// Returns the original work when admission is full, allowing the caller to
    /// preserve upstream backpressure instead of discarding the request.
    pub fn try_push(&mut self, work: T) -> Result<(), T> {
        if self.is_full() {
            self.statistics.rejected = self.statistics.rejected.saturating_add(1);
            return Err(work);
        }
        self.pending.push_back((work, Instant::now()));
        self.statistics.admitted = self.statistics.admitted.saturating_add(1);
        Ok(())
    }

    /// Dispatch the earliest runnable operation. A blocked operation reserves its
    /// resources against later work, so a multi-resource transfer cannot starve.
    /// Unrelated work may bypass it, subject to the same total concurrency bound.
    pub fn next_ready(
        &mut self,
        mut scope: impl FnMut(&T) -> DispatchScope<K>,
    ) -> Option<Dispatched<T>> {
        if self.active.len() >= self.concurrency {
            return None;
        }
        let mut readers = HashSet::new();
        let mut writers = HashSet::new();
        for active in self.active.values() {
            let (shared, exclusive) = active.access()?;
            readers.extend(shared.iter().cloned());
            writers.extend(exclusive.iter().cloned());
        }
        for (index, (work, _)) in self.pending.iter().enumerate() {
            let resources = scope(work);
            if let Some((shared, exclusive)) = resources.access() {
                if shared.iter().any(|key| writers.contains(key))
                    || exclusive
                        .iter()
                        .any(|key| readers.contains(key) || writers.contains(key))
                {
                    readers.extend(shared.iter().cloned());
                    writers.extend(exclusive.iter().cloned());
                    continue;
                }
            } else if !self.active.is_empty() || index != 0 {
                return None;
            }
            let id = DispatchId(self.next_id);
            self.next_id = self.next_id.checked_add(1).expect("dispatch ID exhausted");
            self.active.insert(id, resources);
            let (work, queued_at) = self.pending.remove(index).expect("pending work");
            let now = Instant::now();
            let queued_for = now.duration_since(queued_at);
            self.started_at.insert(id, now);
            self.statistics.started = self.statistics.started.saturating_add(1);
            self.statistics.wait_total = self.statistics.wait_total.saturating_add(queued_for);
            self.statistics.wait_max = self.statistics.wait_max.max(queued_for);
            return Some(Dispatched {
                id,
                work,
                queued_for,
            });
        }
        None
    }

    /// Release reservations only after the worker has stopped using them.
    pub fn finish(&mut self, id: DispatchId) -> bool {
        if self.active.remove(&id).is_none() {
            return false;
        }
        let elapsed = self
            .started_at
            .remove(&id)
            .expect("active dispatch timestamp")
            .elapsed();
        self.statistics.completed = self.statistics.completed.saturating_add(1);
        self.statistics.execution_total = self.statistics.execution_total.saturating_add(elapsed);
        self.statistics.execution_max = self.statistics.execution_max.max(elapsed);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_follow_admission_dispatch_and_single_retirement() {
        let mut queue = DispatchQueue::<&str, &str>::new(2, 1);
        queue.try_push("a").unwrap();
        queue.try_push("b").unwrap();
        assert!(queue.try_push("overflow").is_err());
        let stats = queue.statistics();
        assert_eq!(
            (stats.pending, stats.active, stats.admitted, stats.rejected),
            (2, 0, 2, 1)
        );
        let first = queue
            .next_ready(|_| DispatchScope::Keys(vec!["same"]))
            .unwrap();
        assert!(queue.next_ready(|_| panic!("workers full")).is_none());
        let stats = queue.statistics();
        assert_eq!(
            (stats.pending, stats.active, stats.started, stats.completed),
            (1, 1, 1, 0)
        );
        assert!(stats.wait_total >= first.queued_for);
        assert!(queue.finish(first.id));
        assert!(!queue.finish(first.id));
        let stats = queue.statistics();
        assert_eq!((stats.pending, stats.active, stats.completed), (1, 0, 1));
        assert!(stats.execution_total > std::time::Duration::ZERO);
        assert!(stats.oldest_pending > std::time::Duration::ZERO);
    }

    #[test]
    fn routing_is_needed_only_with_pending_work_and_available_workers() {
        let mut queue = DispatchQueue::<&str, &str>::new(4, 1);
        assert!(!queue.needs_routing());
        queue.try_push("first").unwrap();
        assert!(queue.needs_routing());
        let first = queue
            .next_ready(|_| DispatchScope::Keys(vec!["a"]))
            .unwrap();
        assert!(!queue.needs_routing());
        queue.try_push("second").unwrap();
        assert!(!queue.needs_routing());
        queue.finish(first.id);
        assert!(queue.needs_routing());
        let second = queue
            .next_ready(|_| DispatchScope::Keys(vec!["b"]))
            .unwrap();
        queue.finish(second.id);
        assert!(!queue.needs_routing());
    }

    fn scope(job: &(&str, Vec<&str>)) -> DispatchScope<String> {
        DispatchScope::Keys(job.1.iter().map(|key| (*key).to_owned()).collect())
    }

    #[test]
    fn backend_readers_run_together_but_an_exclusive_transition_fences_that_backend() {
        let mut queue = DispatchQueue::new(5, 4);
        for job in ["a", "b", "raw", "c", "other"] {
            queue.try_push(job).unwrap();
        }
        let scope = |job: &&str| {
            if *job == "raw" {
                DispatchScope::Keys(vec!["codex".to_owned()])
            } else {
                DispatchScope::Access {
                    shared: vec![if *job == "other" { "pi" } else { "codex" }.to_owned()],
                    exclusive: vec![job.to_string()],
                }
            }
        };
        let a = queue.next_ready(scope).unwrap();
        let b = queue.next_ready(scope).unwrap();
        assert_eq!((a.work, b.work), ("a", "b"));
        assert_eq!(queue.next_ready(scope).unwrap().work, "other");
        queue.finish(a.id);
        assert!(queue.next_ready(scope).is_none());
        queue.finish(b.id);
        let raw = queue.next_ready(scope).unwrap();
        assert_eq!(raw.work, "raw");
        assert!(queue.next_ready(scope).is_none());
        queue.finish(raw.id);
        assert_eq!(queue.next_ready(scope).unwrap().work, "c");
    }

    #[test]
    fn blocked_conversation_does_not_block_an_independent_one_or_reorder_its_own_work() {
        let mut queue = DispatchQueue::new(4, 2);
        for job in [("slow", vec!["a"]), ("a-next", vec!["a"]), ("b", vec!["b"])] {
            queue.try_push(job).unwrap();
        }
        let slow = queue.next_ready(scope).unwrap();
        assert_eq!(slow.work.0, "slow");
        let other = queue.next_ready(scope).unwrap();
        assert_eq!(other.work.0, "b");
        assert!(queue.next_ready(scope).is_none());
        assert!(queue.finish(other.id));
        assert!(queue.next_ready(scope).is_none());
        assert!(queue.finish(slow.id));
        let next = queue.next_ready(scope).unwrap();
        assert_eq!(next.work.0, "a-next");
        assert!(queue.finish(next.id));
        assert!(queue.is_empty());
        assert!(!queue.finish(next.id));
    }

    #[test]
    fn transfer_reserves_both_sessions_without_blocking_unrelated_work() {
        let mut queue = DispatchQueue::new(5, 3);
        for job in [
            ("old", vec!["old"]),
            ("move", vec!["old", "new"]),
            ("new-event", vec!["new"]),
            ("independent", vec!["other"]),
        ] {
            queue.try_push(job).unwrap();
        }
        let old = queue.next_ready(scope).unwrap();
        assert_eq!(queue.next_ready(scope).unwrap().work.0, "independent");
        assert!(queue.next_ready(scope).is_none());
        queue.finish(old.id);
        let transfer = queue.next_ready(scope).unwrap();
        assert_eq!(transfer.work.0, "move");
        assert!(queue.next_ready(scope).is_none());
        queue.finish(transfer.id);
        assert_eq!(queue.next_ready(scope).unwrap().work.0, "new-event");
    }

    #[test]
    fn capacity_includes_running_work_and_resources_are_resolved_when_dispatching() {
        let mut queue = DispatchQueue::new(2, 2);
        queue.try_push("first").unwrap();
        queue.try_push("second").unwrap();
        let first = queue
            .next_ready(|_| DispatchScope::Keys(vec!["session-a"]))
            .unwrap();
        assert_eq!(queue.try_push("overflow"), Err("overflow"));
        assert!(
            queue
                .next_ready(|_| DispatchScope::Keys(vec!["session-a"]))
                .is_none()
        );
        // A preceding attachment changed the pending job's resource. It must not
        // retain the resource calculated when the job was enqueued.
        let next = queue
            .next_ready(|_| DispatchScope::Keys(vec!["session-b"]))
            .unwrap();
        assert_eq!(next.work, "second");
        queue.finish(first.id);
        queue.try_push("third").unwrap();
    }

    #[test]
    fn global_recovery_barrier_waits_for_running_work_and_fences_later_work() {
        let mut queue = DispatchQueue::new(4, 3);
        for item in ["a", "recovery", "b"] {
            queue.try_push(item).unwrap();
        }
        let scope = |item: &&str| {
            if *item == "recovery" {
                DispatchScope::Global
            } else {
                DispatchScope::Keys(vec![item.to_string()])
            }
        };
        let a = queue.next_ready(scope).unwrap();
        assert!(queue.next_ready(scope).is_none());
        queue.finish(a.id);
        let barrier = queue.next_ready(scope).unwrap();
        assert_eq!(barrier.work, "recovery");
        assert!(queue.next_ready(scope).is_none());
        queue.finish(barrier.id);
        assert_eq!(queue.next_ready(scope).unwrap().work, "b");
    }
}
