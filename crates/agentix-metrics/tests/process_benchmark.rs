//! The production reader is shared directly; this binary has no allocation-counting allocator.
#![cfg(target_os = "macos")]
#[path = "../src/resources.rs"]
mod resources;
use std::{hint::black_box, time::Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

#[test]
#[ignore = "manual release native process sampling measurement"]
#[cfg(target_os = "macos")]
fn own_process_sampling_benchmark() {
    let mut reader = resources::Reader::new().unwrap();
    let mut system = System::new();
    let pid = sysinfo::get_current_pid().unwrap();
    let mut baseline = || {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cpu()
                .with_memory(),
        );
        let p = system.process(pid).unwrap();
        black_box((
            p.accumulated_cpu_time(),
            p.memory(),
            p.virtual_memory(),
            p.start_time(),
            p.run_time(),
        ));
    };
    baseline();
    let initial = reader.sample().unwrap();
    assert!(initial.cpu.as_secs_f64() >= 0.0 && initial.resident > 0 && initial.virtual_memory > 0);
    assert!(initial.start > 0);
    black_box(initial.uptime);
    for round in 0..7 {
        for direct in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            for _ in 0..1000 {
                if direct {
                    black_box(reader.sample().unwrap());
                } else {
                    baseline();
                }
            }
            println!(
                "{{\"direct\":{direct},\"round\":{round},\"samples\":1000,\"elapsed_ns\":{}}}",
                started.elapsed().as_nanos()
            );
        }
    }
}
