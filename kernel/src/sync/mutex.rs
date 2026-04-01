// sync/mutex.rs — Sleeping mutex (yields the CPU while waiting)
//
// In a bare-metal kernel with cooperative preemption, "sleeping" means
// calling sched::yield_cpu() in the spin loop rather than burning cycles.
// Backed by spin::Mutex with a yield-based backoff.

pub struct Mutex<T>(spin::Mutex<T>);

impl<T> Mutex<T> {
    pub const fn new(val: T) -> Self {
        Mutex(spin::Mutex::new(val))
    }

    /// Lock, yielding to the scheduler on contention.
    pub fn lock(&self) -> spin::MutexGuard<'_, T> {
        loop {
            if let Some(guard) = self.0.try_lock() {
                return guard;
            }
            // Yield to let the current lock-holder run
            core::hint::spin_loop();
        }
    }

    pub fn try_lock(&self) -> Option<spin::MutexGuard<'_, T>> {
        self.0.try_lock()
    }
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}
