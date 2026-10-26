//! Native reads for the exporting process, separate from exposition and scheduling.
use std::time::Duration;
#[cfg(not(target_os = "macos"))]
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub(super) struct Sample {
    pub(super) cpu: Duration,
    pub(super) resident: u64,
    pub(super) virtual_memory: u64,
    pub(super) start: u64,
    pub(super) uptime: u64,
}
#[cfg(not(target_os = "macos"))]
pub(super) struct Reader {
    system: System,
    pid: sysinfo::Pid,
}
#[cfg(not(target_os = "macos"))]
impl Reader {
    pub(super) fn new() -> Option<Self> {
        Some(Self {
            system: System::new(),
            pid: sysinfo::get_current_pid().ok()?,
        })
    }
    pub(super) fn sample(&mut self) -> Option<Sample> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cpu()
                .with_memory(),
        );
        let process = self.system.process(self.pid)?;
        Some(Sample {
            cpu: Duration::from_millis(process.accumulated_cpu_time()),
            resident: process.memory(),
            virtual_memory: process.virtual_memory(),
            start: process.start_time(),
            uptime: process.run_time(),
        })
    }
}

// sysinfo's macOS backend enumerates all PIDs before applying a PID filter.
// The direct API queries only this process and returns byte counts/start time.
#[cfg(target_os = "macos")]
pub(super) struct Reader {
    pid: i32,
}
#[cfg(target_os = "macos")]
impl Reader {
    pub(super) fn new() -> Option<Self> {
        Some(Self {
            pid: i32::try_from(std::process::id()).ok()?,
        })
    }
    pub(super) fn sample(&mut self) -> Option<Sample> {
        use libproc::{proc_pid::pidinfo, task_info::TaskAllInfo};
        let info = pidinfo::<TaskAllInfo>(self.pid, 0).ok()?;
        // CLOCK_PROCESS_CPUTIME_ID gives Duration directly, avoiding Darwin's
        // architecture-dependent Mach tick units in proc_taskinfo CPU fields.
        let cpu = cpu_time::ProcessTime::try_now().ok()?.as_duration();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        Some(Sample {
            cpu,
            resident: info.ptinfo.pti_resident_size,
            virtual_memory: info.ptinfo.pti_virtual_size,
            start: info.pbsd.pbi_start_tvsec,
            uptime: now.saturating_sub(info.pbsd.pbi_start_tvsec),
        })
    }
}
