// sync/spinlock.rs — Spin-waiting lock backed by spinning_top::Spinlock
//
// spinning_top::Spinlock implements lock_api::RawMutex and is no_std compatible.
// Use SpinLock for data protected in IRQ handlers or very short critical sections.

pub use spinning_top::guard::SpinlockGuard;

pub struct SpinLock<T>(spinning_top::Spinlock<T>);

impl<T> SpinLock<T> {
    pub const fn new(val: T) -> Self {
        SpinLock(spinning_top::Spinlock::new(val))
    }

    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        self.0.lock()
    }

    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        self.0.try_lock()
    }
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}
