use std::sync::Mutex;

use anyhow::{Result, ensure};
use tokio::sync::{Semaphore, SemaphorePermit};

/// Admission shared by all configuration generations of a named provider.
pub struct ProviderLimits {
    pub(super) total: Limit,
    pub(super) background: Limit,
}

impl ProviderLimits {
    pub fn new(capacity: usize) -> Result<Self> {
        ensure!(
            (1..=64).contains(&capacity),
            "invalid: provider concurrency"
        );
        Ok(Self {
            total: Limit::new(capacity),
            background: Limit::new(capacity.saturating_sub(1).max(1)),
        })
    }

    /// Existing requests finish; reductions retire permits before new admission.
    pub fn set_limit(&self, capacity: usize) -> Result<()> {
        ensure!(
            (1..=64).contains(&capacity),
            "invalid: provider concurrency"
        );
        self.total.resize(capacity);
        self.background.resize(capacity.saturating_sub(1).max(1));
        Ok(())
    }
}

struct State {
    capacity: usize,
    target: usize,
}

pub(super) struct Limit {
    semaphore: Semaphore,
    state: Mutex<State>,
}

impl Limit {
    fn new(capacity: usize) -> Self {
        Self {
            semaphore: Semaphore::new(capacity),
            state: Mutex::new(State {
                capacity,
                target: capacity,
            }),
        }
    }

    fn resize(&self, target: usize) {
        let mut state = self.state.lock().expect("admission state poisoned");
        state.target = target;
        if target > state.capacity {
            self.semaphore.add_permits(target - state.capacity);
            state.capacity = target;
        } else {
            state.capacity -= self.semaphore.forget_permits(state.capacity - target);
        }
    }

    pub(super) async fn acquire(&self) -> Result<Permit<'_>> {
        loop {
            let permit = self.semaphore.acquire().await?;
            let mut state = self.state.lock().expect("admission state poisoned");
            // A queued waiter may already own a permit when a reduction occurs.
            if state.capacity > state.target {
                permit.forget();
                state.capacity -= 1;
            } else {
                return Ok(Permit {
                    permit: Some(permit),
                    limit: self,
                });
            }
        }
    }
}

pub(super) struct Permit<'a> {
    permit: Option<SemaphorePermit<'a>>,
    limit: &'a Limit,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self.limit.state.lock().expect("admission state poisoned");
        if let Some(permit) = self.permit.take() {
            if state.capacity > state.target {
                permit.forget();
                state.capacity -= 1;
            } else {
                drop(permit);
            }
        }
    }
}
