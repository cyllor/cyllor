// AArch64 Generic Timer - EL1 Physical Timer

const TIMER_FREQ_HZ: u64 = 10; // 10 Hz tick (100ms)

/// Read counter frequency (CNTFRQ_EL0)
fn cntfrq() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) val) };
    val
}

/// Read current counter value
#[allow(dead_code)]
pub fn counter() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) val) };
    val
}

/// Set up the timer to fire at TIMER_FREQ_HZ
pub fn init() {
    let freq = cntfrq();
    let ticks = freq / TIMER_FREQ_HZ;

    unsafe {
        // Set compare value
        core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) ticks);
        // Enable timer, unmask interrupt
        core::arch::asm!("msr CNTP_CTL_EL0, {}", in(reg) 1u64);
    }

    // Only log from BSP — AP context may not be safe for log
    if crate::sched::cpu::current_cpu_id() == 0 {
        log::debug!("Timer: freq={freq} Hz, tick every {ticks} counts ({} ms)", 1000 / TIMER_FREQ_HZ);
    }
}


/// Reset timer for next tick
pub fn reset() {
    let freq = cntfrq();
    let ticks = freq / TIMER_FREQ_HZ;
    unsafe {
        core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) ticks);
    }
}
