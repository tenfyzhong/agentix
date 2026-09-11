//! Owned, bounded workers for durable task notifications.
use std::{sync::Arc, time::Duration};

use agentix_core::Engine;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const CONCURRENCY: usize = 32;

pub(crate) async fn run(
    snapshots: tokio::sync::watch::Receiver<Arc<Engine>>,
    shutdown: CancellationToken,
) {
    let mut workers = JoinSet::new();
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut admission_first = false;
    loop {
        let engine = snapshots.borrow().clone();
        if shutdown.is_cancelled() {
            break;
        }
        if workers.len() < CONCURRENCY {
            let limit = u32::try_from(CONCURRENCY - workers.len()).unwrap();
            let claimed = tokio::select! {
                () = shutdown.cancelled() => break,
                result = claim(&engine, limit, admission_first) => result,
            };
            match claimed {
                Ok(notifications) => {
                    for notification in notifications {
                        let engine = engine.clone();
                        workers.spawn(async move {
                            engine.deliver_task_notification(notification).await
                        });
                    }
                }
                Err(error) => tracing::warn!(%error, "task notification admission failed"),
            }
            admission_first = !admission_first;
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = poll.tick() => {},
            result = workers.join_next(), if !workers.is_empty() => {
                match result.expect("active notification worker") {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => tracing::warn!(%error, "task notification will retry"),
                    Err(error) => tracing::warn!(%error, "task notification worker stopped; lease recovery will retry"),
                }
            }
        }
    }
    // A stopped worker's lease expires after 60 seconds. No detached send can
    // survive cancellation, and a crash uses the same durable recovery path.
    workers.shutdown().await;
}

async fn claim(
    engine: &Engine,
    limit: u32,
    admission_first: bool,
) -> Result<Vec<agentix_core::TaskNotification>, agentix_core::EngineError> {
    let mut batch = if admission_first {
        engine
            .claim_admission_notifications(limit.div_ceil(2))
            .await?
    } else {
        engine.claim_task_notifications(limit.div_ceil(2)).await?
    };
    let remaining = limit - u32::try_from(batch.len()).unwrap();
    if remaining > 0 {
        batch.extend(if admission_first {
            engine.claim_task_notifications(remaining).await?
        } else {
            engine.claim_admission_notifications(remaining).await?
        });
    }
    Ok(batch)
}
