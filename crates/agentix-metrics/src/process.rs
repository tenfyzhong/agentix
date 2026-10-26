//! Only the exporting process is refreshed; HTTP never queries the operating system.
use crate::resources::Reader;
use crate::{Metrics, Snapshot};
use std::{fmt::Write, time::Duration};
use tokio::task::JoinHandle;

pub(super) struct Sampler(JoinHandle<()>);
impl Drop for Sampler {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn start(metrics: Metrics) -> Option<Sampler> {
    // No PID lookup, system object or task is created when recording is disabled.
    let prefix = metrics.0.as_ref()?.service.prefix();
    let mut reader = Reader::new()?;
    Some(Sampler(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let sample = tokio::task::spawn_blocking(move || {
                let text = reader.sample().map(|process| {
                    let mut text = String::new();
                    let cpu = process.cpu.as_secs_f64();
                    writeln!(text, "# HELP {prefix}_process_cpu_seconds CPU time used by this process\n# TYPE {prefix}_process_cpu_seconds counter\n{prefix}_process_cpu_seconds_total {cpu}").unwrap();
                    for (name, help, value) in [
                        ("resident_memory_bytes", "Resident memory of this process in bytes", process.resident),
                        ("virtual_memory_bytes", "Virtual address space of this process in bytes", process.virtual_memory),
                        ("start_time_seconds", "Process start time as Unix seconds", process.start),
                        ("uptime_seconds", "Elapsed process lifetime in seconds", process.uptime),
                    ] {
                        writeln!(text, "# HELP {prefix}_process_{name} {help}\n# TYPE {prefix}_process_{name} gauge\n{prefix}_process_{name} {value}").unwrap();
                    }
                    text
                });
                (reader, text)
            }).await;
            match sample {
                Ok((next, text)) => {
                    reader = next;
                    if let Some(text) = text {
                        metrics.publish(Snapshot::Process, text);
                    } else {
                        metrics.clear(Snapshot::Process);
                        tracing::warn!("own-process metrics unavailable");
                    }
                }
                Err(error) => {
                    metrics.clear(Snapshot::Process);
                    tracing::warn!(%error, "own-process metrics worker stopped");
                    break;
                }
            }
        }
    })))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn warmed_own_process_sampling_does_not_allocate() {
        let mut reader = super::Reader::new().unwrap();
        reader.sample().unwrap();
        let allocations = allocation_counter::measure(|| {
            for _ in 0..10 {
                let sample = reader.sample().unwrap();
                assert!(sample.resident > 0);
                assert!(sample.start > 0);
            }
        });
        assert_eq!(allocations.count_total, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_sample_matches_process_start_and_cpu_units() {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::get_current_pid().unwrap();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cpu()
                .with_memory(),
        );
        let reference = system.process(pid).unwrap();
        let sample = super::Reader::new().unwrap().sample().unwrap();
        assert_eq!(sample.start, reference.start_time());
        assert!(
            sample.cpu.abs_diff(std::time::Duration::from_millis(
                reference.accumulated_cpu_time()
            )) < std::time::Duration::from_secs(1)
        );
        assert!(sample.resident > 0 && sample.virtual_memory >= sample.resident);
    }

    #[tokio::test]
    async fn dropping_sampler_cancels_its_background_task() {
        let sampler = super::start(crate::Metrics::new(crate::Service::Agentix, "test")).unwrap();
        let task = sampler.0.abort_handle();
        drop(sampler);
        tokio::task::yield_now().await;
        assert!(task.is_finished());
    }

    #[test]
    fn disabled_metrics_do_not_even_require_a_runtime() {
        assert!(super::start(crate::Metrics::default()).is_none());
    }
}
