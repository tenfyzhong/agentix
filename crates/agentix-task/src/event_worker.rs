//! Background ownership belongs to Taskix, with a file lock shared by CLI and library workers.
use crate::Store;
use anyhow::Result;
use std::{
    fs::{File, OpenOptions},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

impl Store {
    /// CLI hosts hand maintenance to a detached Taskix process before their runtime exits.
    pub fn set_background_maintenance(&self, enabled: bool) {
        self.background_enabled.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn schedule_event_maintenance(&self) {
        self.wrote.store(true, Ordering::Relaxed);
        if !self.background_enabled.load(Ordering::Relaxed)
            || self.background_running.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let store = self.clone();
        tokio::spawn(async move {
            if let Err(error) = store.run_event_maintenance().await {
                tracing::warn!(%error, "automatic Taskix event maintenance will retry");
            }
            store.background_running.store(false, Ordering::Release);
        });
    }

    pub async fn maintenance_requested(&self) -> Result<bool> {
        if !self.wrote.load(Ordering::Relaxed) {
            return Ok(false);
        }
        Ok(
            sqlx::query_scalar("SELECT enabled AND next_run_at<=? FROM event_retention WHERE id=1")
                .bind(self.now())
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Nonblocking OS lock; process exit releases ownership, including crashes.
    pub fn try_maintenance_lock(&self) -> Result<Option<File>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.database_path.with_extension("maintenance.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Drain pending work, yielding between batches. No timer remains when idle.
    pub async fn run_event_maintenance(&self) -> Result<()> {
        let Some(_lock) = self.try_maintenance_lock()? else {
            return Ok(());
        };
        let mut started = Instant::now();
        let mut continuing = false;
        loop {
            let report = self.retention_attempt(continuing).await?;
            if report.as_ref().is_some_and(|r| r["more"] != true) {
                break;
            }
            if report.is_none() {
                let policy = self.event_policy(None, None, None).await?;
                if policy["enabled"] != true
                    || (!continuing && policy["next_run_at"].as_i64().unwrap_or(0) > self.now())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            if started.elapsed() >= Duration::from_secs(30) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                started = Instant::now();
            }
            continuing = true;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    pub(crate) async fn enable_incremental_vacuum(&self) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&mut *conn)
            .await?;
        if mode != 2 {
            sqlx::query("PRAGMA auto_vacuum=INCREMENTAL")
                .execute(&mut *conn)
                .await?;
            if mode == 0 {
                // One-time format upgrade, outside any transaction. Never a daily maintenance step.
                sqlx::query("VACUUM").execute(&mut *conn).await?;
            }
        }
        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&mut *conn)
            .await?;
        anyhow::ensure!(
            mode == 2,
            "incremental vacuum conversion did not persist: {mode}"
        );
        Ok(())
    }
}
