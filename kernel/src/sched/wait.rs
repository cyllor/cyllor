//! Deadline-based sleep and wakeup.
//!
//! `tick()` is called every timer interrupt from `sched::timer_tick()`.
//! Threads sleep by calling `sleep_ticks()` / `sleep_ns()`, which delegate to
//! `scheduler::block_current_until()` — marking the thread Blocked, registering
//! its wakeup deadline here, and yielding the CPU.  When the deadline arrives
//! `tick()` sets the thread back to Ready so the scheduler can pick it up.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use super::process::Pid;

/// Monotonically increasing tick counter (incremented by `tick()`).
static TICK: AtomicU64 = AtomicU64::new(0);

/// Timer frequency in Hz — updated by the arch timer driver.
static TICK_HZ: AtomicU64 = AtomicU64::new(100);

/// Pending wakeups: wake_tick → list of PIDs to wake.
static SLEEP_QUEUE: Mutex<BTreeMap<u64, Vec<Pid>>> = Mutex::new(BTreeMap::new());

/// Set the timer frequency (called from arch timer init).
pub fn set_tick_hz(hz: u64) {
    TICK_HZ.store(hz, Ordering::Relaxed);
}

/// Current tick count.
pub fn current_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

/// Advance the tick counter and wake all threads whose deadline has passed.
/// Must be called from `sched::timer_tick()` on every timer interrupt.
pub fn tick() {
    let now = TICK.fetch_add(1, Ordering::Relaxed) + 1;

    let mut to_wake: Vec<Pid> = Vec::new();
    {
        let mut q = SLEEP_QUEUE.lock();
        while let Some((&wake_at, _)) = q.iter().next() {
            if wake_at > now { break; }
            if let Some(pids) = q.remove(&wake_at) {
                to_wake.extend_from_slice(&pids);
            }
        }
    }

    if to_wake.is_empty() { return; }

    let mut sched = super::scheduler::SCHEDULER.lock();
    for pid in to_wake {
        // Search every CPU's run queue for this Blocked thread and wake it.
        'outer: for cpu in 0..sched.num_cpus {
            for thread in sched.run_queues[cpu].iter_mut() {
                if thread.pid == pid
                    && thread.state == super::process::ThreadState::Blocked
                {
                    thread.state = super::process::ThreadState::Ready;
                    break 'outer;
                }
            }
        }
    }
}

/// Register `pid` to be woken at `wake_tick`.
pub fn register_wakeup(pid: Pid, wake_tick: u64) {
    SLEEP_QUEUE.lock().entry(wake_tick).or_default().push(pid);
}

/// Cancel any pending wakeup for `pid`.
pub fn cancel_wakeup(pid: Pid) {
    let mut q = SLEEP_QUEUE.lock();
    for pids in q.values_mut() {
        pids.retain(|&p| p != pid);
    }
}

/// Sleep for `n` timer ticks.
/// Marks the current thread Blocked and yields; returns once woken.
pub fn sleep_ticks(n: u64) {
    if n == 0 { return; }
    let wake_at = current_tick() + n;
    super::scheduler::block_current_until(wake_at);
}

/// Sleep for approximately `ns` nanoseconds.
pub fn sleep_ns(ns: u64) {
    let hz = TICK_HZ.load(Ordering::Relaxed).max(1);
    // Round up to at least one tick.
    let ticks = ((ns * hz) + 999_999_999) / 1_000_000_000;
    sleep_ticks(ticks.max(1));
}
