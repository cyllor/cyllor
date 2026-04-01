//! Per-CPU data and CPU-ID helpers.

use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_CPUS: usize = 8;

/// PID of the thread currently running on each CPU (0 = idle).
static CURRENT_PID: [AtomicU64; MAX_CPUS] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; MAX_CPUS]
};

/// Return the logical CPU ID from MPIDR.Aff0.
pub fn current_cpu_id() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        let mpidr: u64;
        unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
        (mpidr & 0xFF) as usize
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 0 }
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
