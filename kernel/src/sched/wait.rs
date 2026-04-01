// sched/wait.rs — sleep and wait infrastructure
//
// Provides deadline-based sleeping for kernel and user threads.
// The tick counter is driven by timer_tick() in sched/mod.rs.
//
// Usage:
//   sched::wait::sleep_ticks(5);          // sleep for 5 timer ticks
//   sched::wait::sleep_ns(1_000_000);     // sleep for ~1 ms

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use super::process::Pid;

/// Monotonically increasing tick counter (incremented by tick()).
static TICK: AtomicU64 = AtomicU64::new(0);

/// Current timer frequency in Hz (set during init, default 100 Hz = 10 ms ticks).
static TICK_HZ: AtomicU64 = AtomicU64::new(100);

/// Sleeping threads: wake_tick → list of PIDs to wake.
static SLEEP_QUEUE: Mutex<BTreeMap<u64, Vec<Pid>>> = Mutex::new(BTreeMap::new());

/// Set the timer frequency (used by arch timer init).
pub fn set_tick_hz(hz: u64) {
    TICK_HZ.store(hz, Ordering::Relaxed);
}

/// Current tick count.
pub fn current_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

/// Advance the tick counter and wake any threads whose deadline has passed.
/// Called from `timer_tick()` in sched/mod.rs.
pub fn tick() {
    let now = TICK.fetch_add(1, Ordering::Relaxed) + 1;

    // Collect PIDs whose wake time has arrived
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

    // Mark woken threads as Ready in the run queue.
    // Threads sleeping via sleep_ticks() are blocked using ThreadState::Blocked.
    // We move them back to their CPU's run queue here.
    if !to_wake.is_empty() {
        let mut sched = super::scheduler::SCHEDULER.lock();
        for pid in to_wake {
            // Find the blocked thread in any CPU's run queue or current slot and unblock it.
            // (Blocked threads stay in their run queue with state = Blocked during sleep.)
            for cpu in 0..sched.num_cpus {
                // Iterate values_mut to find and flip state
                for thread in sched.run_queues[cpu].values_mut() {
                    if thread.pid == pid {
                        if thread.state == super::process::ThreadState::Blocked {
                            thread.state = super::process::ThreadState::Ready;
                        }
                        break;
                    }
                }
            }
        }
    }
}

/// Sleep for `n` timer ticks.  The calling thread is marked Blocked and
/// will be rescheduled once `n` ticks have elapsed.
///
/// This is a spinning sleep that yields via `crate::sched::timer_tick()`
/// rather than actually de-scheduling, keeping the implementation simple
/// until a proper block/unblock mechanism is added.
pub fn sleep_ticks(n: u64) {
    if n == 0 { return; }
    let wake_at = current_tick() + n;
    loop {
        if current_tick() >= wake_at { return; }
        crate::sched::timer_tick();
    }
}

/// Sleep for approximately `ns` nanoseconds.
pub fn sleep_ns(ns: u64) {
    let hz = TICK_HZ.load(Ordering::Relaxed).max(1);
    // ticks = ns * hz / 1_000_000_000 (round up)
    let ticks = (ns * hz + 999_999_999) / 1_000_000_000;
    sleep_ticks(ticks.max(1));
}

/// Register a PID to be woken at `wake_tick`.
/// Used by the futex/waitqueue subsystem for deadline-based wakeups.
pub fn register_wakeup(pid: Pid, wake_tick: u64) {
    SLEEP_QUEUE.lock().entry(wake_tick).or_default().push(pid);
}

/// Cancel a pending wakeup for `pid`.
pub fn cancel_wakeup(pid: Pid) {
    let mut q = SLEEP_QUEUE.lock();
    for pids in q.values_mut() {
        pids.retain(|&p| p != pid);
    }
}
