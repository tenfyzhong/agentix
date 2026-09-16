//! Per-operation transport progress. Scopes stay in the caller future; adapters
//! must mark dispatch in that same task and only re-enter waiting after rejection.
use crate::ChannelError;
use std::{future::Future, time::Duration};
use tokio::{sync::watch, time::Instant};

const OPERATION_BUDGET: Duration = Duration::from_mins(1);
const WIRE_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum Phase {
    Waiting,
    InFlight(Instant),
}

tokio::task_local! { static ATTEMPT: DeliveryAttempt; }

#[derive(Clone)]
pub struct DeliveryAttempt {
    phase: watch::Sender<Phase>,
}
impl Default for DeliveryAttempt {
    fn default() -> Self {
        Self {
            phase: watch::channel(Phase::InFlight(Instant::now())).0,
        }
    }
}
impl DeliveryAttempt {
    /// The operation is queued locally, or the previous attempt was rejected.
    pub fn waiting() {
        let _ = ATTEMPT.try_with(|attempt| attempt.phase.send_replace(Phase::Waiting));
    }
    /// Immediately before polling a request that can reach the provider.
    pub fn dispatched() {
        let _ =
            ATTEMPT.try_with(|attempt| attempt.phase.send_replace(Phase::InFlight(Instant::now())));
    }
    #[must_use]
    pub fn not_dispatched(&self) -> bool {
        matches!(*self.phase.borrow(), Phase::Waiting)
    }

    /// Bound total admission/retry time separately from each remote request.
    /// Adapters without progress instrumentation conservatively count as in flight.
    pub async fn run<T>(
        &self,
        operation: impl Future<Output = Result<T, ChannelError>>,
    ) -> Result<T, ChannelError> {
        let overall = Instant::now() + OPERATION_BUDGET;
        let mut phase = self.phase.subscribe();
        let operation = ATTEMPT.scope(self.clone(), operation);
        tokio::pin!(operation);
        loop {
            let deadline = match *phase.borrow_and_update() {
                Phase::Waiting => overall,
                Phase::InFlight(start) => overall.min(start + WIRE_BUDGET),
            };
            tokio::select! {
                biased;
                result = &mut operation => return match result {
                    Err(ChannelError::Transport(message)) if self.not_dispatched() => Err(ChannelError::NotSent(message)),
                    result => result,
                },
                changed = phase.changed() => { changed.expect("attempt owns sender"); }
                () = tokio::time::sleep_until(deadline) => {
                    return Err(if self.not_dispatched() {
                        ChannelError::NotSent("local delivery wait exceeded its budget".into())
                    } else {
                        ChannelError::Transport("remote delivery response timed out".into())
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_transport_failure_before_dispatch_is_known_unsent() {
        let result = DeliveryAttempt::default()
            .run(async {
                DeliveryAttempt::waiting();
                Err::<(), _>(ChannelError::Transport("local admission failed".into()))
            })
            .await;
        assert!(matches!(result, Err(ChannelError::NotSent(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn local_wait_has_separate_budget_from_remote_request() {
        let attempt = DeliveryAttempt::default();
        let start = Instant::now();
        attempt
            .run(async {
                DeliveryAttempt::waiting();
                tokio::time::sleep(Duration::from_secs(30)).await;
                DeliveryAttempt::dispatched();
                tokio::time::sleep(Duration::from_secs(4)).await;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(34));
    }

    #[tokio::test(start_paused = true)]
    async fn deadlines_distinguish_unsent_and_unknown_remote_outcomes() {
        let attempt = DeliveryAttempt::default();
        let result = attempt
            .run(async {
                DeliveryAttempt::waiting();
                std::future::pending::<Result<(), ChannelError>>().await
            })
            .await;
        assert!(matches!(result, Err(ChannelError::NotSent(_))));
        let attempt = DeliveryAttempt::default();
        let start = Instant::now();
        let result = attempt
            .run(std::future::pending::<Result<(), ChannelError>>())
            .await;
        assert!(matches!(result, Err(ChannelError::Transport(_))));
        assert_eq!(start.elapsed(), WIRE_BUDGET);
    }

    #[tokio::test(start_paused = true)]
    async fn rejection_retry_wait_does_not_expire_previous_wire_deadline() {
        let attempt = DeliveryAttempt::default();
        attempt
            .run(async {
                DeliveryAttempt::dispatched();
                tokio::time::sleep(Duration::from_secs(4)).await;
                DeliveryAttempt::waiting();
                tokio::time::sleep(Duration::from_secs(30)).await;
                DeliveryAttempt::dispatched();
                tokio::time::sleep(Duration::from_secs(4)).await;
                Ok(())
            })
            .await
            .unwrap();
    }
}
