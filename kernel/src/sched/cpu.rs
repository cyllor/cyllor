// Per-CPU data
// Currently CPU ID is derived from MPIDR_EL1 directly
// This module will be expanded for per-CPU runqueues, idle threads, etc.

pub fn current_cpu_id() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        let mpidr: u64;
        unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
        (mpidr & 0xFF) as usize
    }
    #[cfg(target_arch = "x86_64")]
    {
        0 // TODO: read LAPIC ID
    }
}
