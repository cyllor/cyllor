use crate::syscall::{SyscallResult, EAGAIN, EFAULT, EINVAL, ETIMEDOUT};
use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;

// Futex operations
const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_REQUEUE: i32 = 3;
const FUTEX_CMP_REQUEUE: i32 = 4;
const FUTEX_WAIT_BITSET: i32 = 9;
const FUTEX_WAKE_BITSET: i32 = 10;
const FUTEX_CMD_MASK: i32 = 0x7f;

static FUTEX_WAITERS: Mutex<BTreeMap<u64, VecDeque<u64>>> = Mutex::new(BTreeMap::new()); // uaddr -> tids
static FUTEX_TOKENS: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new()); // tid -> wake tokens

fn read_user_u32(addr: u64) -> Result<u32, i32> {
    let mut raw = [0u8; 4];
    crate::syscall::fs::copy_from_user(addr, &mut raw).map_err(|_| EFAULT)?;
    Ok(u32::from_le_bytes(raw))
}

fn read_timeout_ticks(timeout_ptr: u64) -> Result<u64, i32> {
    if timeout_ptr == 0 {
        return Ok(0);
    }
    let mut raw = [0u8; 16];
    crate::syscall::fs::copy_from_user(timeout_ptr, &mut raw).map_err(|_| EFAULT)?;
    let secs = u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]);
    let nsecs = u64::from_le_bytes([
        raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14], raw[15],
    ]);
    let freq = crate::arch::counter_freq();
    Ok(secs
        .saturating_mul(freq)
        .saturating_add(nsecs.saturating_mul(freq) / 1_000_000_000))
}

fn to_sched_ticks(counter_ticks: u64) -> u64 {
    let counter_hz = crate::arch::counter_freq().max(1);
    let sched_hz = 100u64;
    ((counter_ticks.saturating_mul(sched_hz)).saturating_add(counter_hz - 1)) / counter_hz
}

fn wake_tid(tid: u64) -> bool {
    let mut sched = crate::sched::scheduler::SCHEDULER.lock();
    for cpu in 0..sched.num_cpus {
        if let Some(curr) = sched.current[cpu].as_mut() {
            if curr.pid == tid && curr.state == crate::sched::process::ThreadState::Blocked {
                curr.state = crate::sched::process::ThreadState::Ready;
                return true;
            }
        }
        for th in sched.run_queues[cpu].iter_mut() {
            if th.pid == tid && th.state == crate::sched::process::ThreadState::Blocked {
                th.state = crate::sched::process::ThreadState::Ready;
                return true;
            }
        }
    }
    false
}

pub fn do_futex(
    uaddr: u64,
    futex_op: i32,
    val: u32,
    timeout: u64,
    uaddr2: u64,
    val3: u32,
) -> SyscallResult {
    if uaddr == 0 {
        return Err(EINVAL);
    }

    let _ = uaddr2;
    let op = futex_op & FUTEX_CMD_MASK;

    match op {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            if op == FUTEX_WAIT_BITSET && val3 == 0 {
                return Err(EINVAL);
            }
            let current = read_user_u32(uaddr)?;
            if current != val {
                return Err(EAGAIN);
            }

            let tid = crate::sched::process::current_pid();
            let timeout_ticks = if timeout != 0 {
                to_sched_ticks(read_timeout_ticks(timeout)?)
            } else {
                0
            };
            if timeout != 0 && timeout_ticks == 0 {
                return Err(ETIMEDOUT);
            }

            {
                let mut waiters = FUTEX_WAITERS.lock();
                waiters.entry(uaddr).or_default().push_back(tid);
            }

            let wake_at = if timeout != 0 {
                crate::sched::wait::current_tick().saturating_add(timeout_ticks)
            } else {
                u64::MAX
            };
            crate::sched::scheduler::block_current_until(wake_at);

            // If still queued here, we woke by timeout/spurious scheduler wake.
            {
                let mut waiters = FUTEX_WAITERS.lock();
                if let Some(q) = waiters.get_mut(&uaddr) {
                    q.retain(|&p| p != tid);
                    if q.is_empty() {
                        waiters.remove(&uaddr);
                    }
                }
            }

            let was_woken = {
                let mut tokens = FUTEX_TOKENS.lock();
                if let Some(t) = tokens.get_mut(&tid) {
                    if *t > 0 {
                        *t -= 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if was_woken || read_user_u32(uaddr)? != val {
                return Ok(0);
            }
            if timeout != 0 {
                return Err(ETIMEDOUT);
            }
            Ok(0)
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if op == FUTEX_WAKE_BITSET && val3 == 0 {
                return Err(EINVAL);
            }
            if val == 0 {
                return Ok(0);
            }
            let mut woke = 0usize;
            let mut picked = alloc::vec::Vec::new();
            {
                let mut waiters = FUTEX_WAITERS.lock();
                if let Some(q) = waiters.get_mut(&uaddr) {
                    while woke < val as usize {
                        if let Some(tid) = q.pop_front() {
                            picked.push(tid);
                            woke += 1;
                        } else {
                            break;
                        }
                    }
                    if q.is_empty() {
                        waiters.remove(&uaddr);
                    }
                }
            }
            for tid in picked {
                crate::sched::wait::cancel_wakeup(tid);
                if wake_tid(tid) {
                    let mut tokens = FUTEX_TOKENS.lock();
                    let ent = tokens.entry(tid).or_insert(0);
                    *ent = ent.saturating_add(1);
                }
            }
            Ok(woke)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            if uaddr2 == 0 {
                return Err(EINVAL);
            }
            let nr_wake = val as usize;
            let nr_requeue = timeout as usize;
            if op == FUTEX_CMP_REQUEUE {
                let cur = read_user_u32(uaddr)?;
                if cur != val3 {
                    return Err(EAGAIN);
                }
            }

            let mut woke = 0usize;
            let mut picked = alloc::vec::Vec::new();
            let mut moved_tids = alloc::vec::Vec::new();
            {
                let mut waiters = FUTEX_WAITERS.lock();
                let src = waiters.entry(uaddr).or_default();
                while woke < nr_wake {
                    if let Some(tid) = src.pop_front() {
                        picked.push(tid);
                        woke += 1;
                    } else {
                        break;
                    }
                }
                let mut moved = 0usize;
                while moved < nr_requeue {
                    if let Some(tid) = src.pop_front() {
                        moved_tids.push(tid);
                        moved += 1;
                    } else {
                        break;
                    }
                }
                if src.is_empty() {
                    waiters.remove(&uaddr);
                }
                if !moved_tids.is_empty() {
                    let dst = waiters.entry(uaddr2).or_default();
                    for tid in moved_tids.drain(..) {
                        dst.push_back(tid);
                    }
                }
            }
            for tid in picked {
                crate::sched::wait::cancel_wakeup(tid);
                if wake_tid(tid) {
                    let mut tokens = FUTEX_TOKENS.lock();
                    let ent = tokens.entry(tid).or_insert(0);
                    *ent = ent.saturating_add(1);
                }
            }
            Ok(woke)
        }
        _ => Err(EINVAL),
    }
}
