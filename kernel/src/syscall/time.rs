use super::{SyscallResult, EINVAL};

pub fn sys_clock_gettime(clk_id: u32, tp: u64) -> SyscallResult {
    let counter: u64;
    let freq: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) counter);
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq);
    }

    let secs = counter / freq;
    let nsecs = ((counter % freq) * 1_000_000_000) / freq;

    if tp != 0 {
        let mut data = [0u8; 16];
        data[0..8].copy_from_slice(&secs.to_le_bytes());
        data[8..16].copy_from_slice(&nsecs.to_le_bytes());
        let _ = super::fs::copy_to_user(tp, &data);
    }

    Ok(0)
}

pub fn sys_nanosleep(req: u64, rem: u64) -> SyscallResult {
    if req == 0 {
        return Err(EINVAL);
    }

    let secs = unsafe { *(req as *const u64) };
    let nsecs = unsafe { *((req + 8) as *const u64) };

    let freq: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq) };

    let ticks_to_wait = secs * freq + (nsecs * freq) / 1_000_000_000;

    let start: u64;
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) start) };

    loop {
        let now: u64;
        unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) now) };
        if now - start >= ticks_to_wait {
            break;
        }
        core::hint::spin_loop();
    }

    if rem != 0 {
        unsafe {
            *(rem as *mut u64) = 0;
            *((rem + 8) as *mut u64) = 0;
        }
    }

    Ok(0)
}

pub fn sys_clock_nanosleep(clk_id: u32, flags: u32, req: u64, rem: u64) -> SyscallResult {
    sys_nanosleep(req, rem)
}
