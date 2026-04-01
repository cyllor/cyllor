use crate::syscall::{SyscallResult, EINVAL, EAGAIN, ENOSYS};

// Futex operations
const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_PRIVATE_FLAG: i32 = 128;

pub fn do_futex(uaddr: u64, futex_op: i32, val: u32, timeout: u64, uaddr2: u64, val3: u32) -> SyscallResult {
    let op = futex_op & !FUTEX_PRIVATE_FLAG;

    match op {
        FUTEX_WAIT => {
            // Check if *uaddr == val, if so sleep
            let current = unsafe { *(uaddr as *const u32) };
            if current != val {
                return Err(EAGAIN);
            }
            // Simple busy-wait with timeout
            if timeout != 0 {
                let secs = unsafe { *(timeout as *const u64) };
                let nsecs = unsafe { *((timeout + 8) as *const u64) };
                let freq = crate::arch::counter_freq();
                let ticks = secs * freq + (nsecs * freq) / 1_000_000_000;
                let start = crate::arch::read_counter();
                loop {
                    let current = unsafe { *(uaddr as *const u32) };
                    if current != val { return Ok(0); }
                    if crate::arch::read_counter() - start >= ticks { return Ok(0); }
                    core::hint::spin_loop();
                }
            }
            Ok(0)
        }
        FUTEX_WAKE => {
            // Wake up to `val` waiters — in our simple model just return val
            Ok(val as usize)
        }
        _ => {
            // Other futex ops: REQUEUE, CMP_REQUEUE, etc. — stub
            Ok(0)
        }
    }
}
