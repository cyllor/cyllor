//! Per-CPU data and CPU-ID helpers.

use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_CPUS: usize = 8;

/// PID of the thread currently running on each CPU (0 = idle).
static CURRENT_PID: [AtomicU64; MAX_CPUS] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; MAX_CPUS]
};

/// Return the logical CPU ID of the current CPU.
pub fn current_cpu_id() -> usize {
    crate::arch::current_cpu_id()
}

/// Store the PID of the thread now running on `cpu`.
pub fn set_current_pid(cpu: usize, pid: u64) {
    if cpu < MAX_CPUS {
        CURRENT_PID[cpu].store(pid, Ordering::Relaxed);
    }
}

/// Return the PID of the thread currently running on this CPU.
pub fn get_current_pid() -> u64 {
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        CURRENT_PID[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}
