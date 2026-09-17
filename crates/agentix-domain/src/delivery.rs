//! Per-operation transport progress. Scopes stay in the caller future; adapters
//! must mark dispatch in that same task and only re-enter waiting after rejection.
use crate::ChannelError;
use std::{future::Future, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::Instant,
};

const OPERATION_BUDGET: Duration = Duration::from_mins(1);
const WIRE_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum Phase {
    Waiting,
    Invalidated,
    InFlight(Instant),
}

tokio::task_local! { static ATTEMPT: DeliveryAttempt; }
tokio::task_local! { static DISPATCH_CHECK: mpsc::Sender<oneshot::Sender<()>>; }

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
    pub async fn dispatched() {
        if let Ok(check) = DISPATCH_CHECK.try_with(Clone::clone) {
            Self::waiting();
            let (permit, admitted) = oneshot::channel();
            // Only receiving an already validated permit bypasses cooperative
            // yielding. Admission and validation themselves remain cooperative.
            if check.send(permit).await.is_err()
                || tokio::task::coop::unconstrained(admitted).await.is_err()
            {
                // The owning run_if future returns the rejection and drops us.
                std::future::pending::<()>().await;
            }
        }
        let _ =
            ATTEMPT.try_with(|attempt| attempt.phase.send_replace(Phase::InFlight(Instant::now())));
    }
    #[must_use]
    pub fn not_dispatched(&self) -> bool {
        matches!(*self.phase.borrow(), Phase::Waiting | Phase::Invalidated)
    }

    /// The caller's scope check rejected an undispatched operation.
    #[must_use]
    pub fn invalidated(&self) -> bool {
        matches!(*self.phase.borrow(), Phase::Invalidated)
    }

    /// Validate at every instrumented dispatch boundary, including retries.
    /// Both validation and transport remain borrowed, owned, and budgeted here.
    pub async fn run_if<T, F, Fut>(
        &self,
        operation: impl Future<Output = Result<T, ChannelError>>,
        valid: F,
    ) -> Result<T, ChannelError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = bool>,
    {
        let (checks, mut requests) = mpsc::channel::<oneshot::Sender<()>>(1);
        self.run(DISPATCH_CHECK.scope(checks, async {
            tokio::pin!(operation);
            loop {
                tokio::select! {
                    biased;
                    result = &mut operation => return result,
                    Some(permit) = requests.recv() => {
                        if !valid().await {
                            self.phase.send_replace(Phase::Invalidated);
                            return Err(ChannelError::NotSent(
                                "delivery scope expired before dispatch".into(),
                            ));
                        }
                        let _ = permit.send(());
                        // select! itself can yield when its cooperative budget
                        // is empty. Consume this permission before re-entering it.
                        let resumed = std::future::poll_fn(|cx| {
                            std::task::Poll::Ready(operation.as_mut().poll(cx))
                        }).await;
                        if let std::task::Poll::Ready(result) = resumed {
                            return result;
                        }
                    }
                }
            }
        }))
        .await
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
                Phase::Waiting | Phase::Invalidated => overall,
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

    async fn assert_no_scheduling_gap_after_validation(exhaust_budget: bool) {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let checked = AtomicBool::new(false);
        let wires = AtomicUsize::new(0);
        let attempt = DeliveryAttempt::default();
        let delivery = attempt.run_if(
            async {
                DeliveryAttempt::waiting();
                DeliveryAttempt::dispatched().await;
                wires.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || async {
                if exhaust_budget {
                    while tokio::task::coop::has_budget_remaining() {
                        tokio::task::consume_budget().await;
                    }
                }
                checked.store(true, Ordering::SeqCst);
                true
            },
        );
        tokio::pin!(delivery);
        std::future::poll_fn(|cx| {
            let result = delivery.as_mut().poll(cx);
            if result.is_pending() {
                assert!(
                    !checked.load(Ordering::SeqCst) || wires.load(Ordering::SeqCst) > 0,
                    "validated permit must not survive a scheduler yield before dispatch"
                );
            }
            result
        })
        .await
        .unwrap();
        assert_eq!(wires.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_consumes_validation_without_a_scheduler_gap() {
        assert_no_scheduling_gap_after_validation(false).await;
    }

    #[tokio::test]
    async fn dispatch_permission_survives_exhausted_cooperative_budget_without_yielding() {
        assert_no_scheduling_gap_after_validation(true).await;
    }

    #[tokio::test]
    async fn repeated_dispatch_checkpoints_still_yield_cooperatively() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let wires = AtomicUsize::new(0);
        let attempt = DeliveryAttempt::default();
        let delivery = attempt.run_if(
            async {
                for _ in 0..512 {
                    DeliveryAttempt::waiting();
                    DeliveryAttempt::dispatched().await;
                    wires.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            },
            || async { true },
        );
        tokio::pin!(delivery);
        let pending = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(delivery.as_mut().poll(cx).is_pending())
        })
        .await;
        assert!(
            pending,
            "checkpoint admission must preserve cooperative scheduling"
        );
        assert!(wires.load(Ordering::SeqCst) < 512);
        delivery.await.unwrap();
        assert_eq!(wires.load(Ordering::SeqCst), 512);
    }

    #[tokio::test]
    async fn retry_rechecks_scope_after_definite_rejection() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let valid = AtomicBool::new(true);
        let wires = AtomicUsize::new(0);
        let attempt = DeliveryAttempt::default();
        let result = attempt
            .run_if(
                async {
                    DeliveryAttempt::waiting();
                    DeliveryAttempt::dispatched().await;
                    wires.fetch_add(1, Ordering::SeqCst);
                    DeliveryAttempt::waiting();
                    valid.store(false, Ordering::SeqCst);
                    DeliveryAttempt::dispatched().await;
                    wires.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                || async { valid.load(Ordering::SeqCst) },
            )
            .await;
        assert!(matches!(result, Err(ChannelError::NotSent(_))));
        assert!(attempt.not_dispatched());
        assert_eq!(wires.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_validation_is_budgeted_and_drops_operation() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Dropped<'a>(&'a AtomicBool);
        impl Drop for Dropped<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = AtomicBool::new(false);
        let start = Instant::now();
        let result = DeliveryAttempt::default()
            .run_if(
                async {
                    let _guard = Dropped(&dropped);
                    DeliveryAttempt::waiting();
                    DeliveryAttempt::dispatched().await;
                    panic!("blocked validation must not dispatch");
                    #[allow(unreachable_code)]
                    Ok(())
                },
                std::future::pending::<bool>,
            )
            .await;
        assert!(matches!(result, Err(ChannelError::NotSent(_))));
        assert_eq!(start.elapsed(), OPERATION_BUDGET);
        assert!(dropped.load(Ordering::SeqCst));
    }

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
                DeliveryAttempt::dispatched().await;
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
                DeliveryAttempt::dispatched().await;
                tokio::time::sleep(Duration::from_secs(4)).await;
                DeliveryAttempt::waiting();
                tokio::time::sleep(Duration::from_secs(30)).await;
                DeliveryAttempt::dispatched().await;
                tokio::time::sleep(Duration::from_secs(4)).await;
                Ok(())
            })
            .await
            .unwrap();
    }
}
