// sync/rwlock.rs — Reader-writer lock backed by spin::RwLock
//
// Multiple concurrent readers OR one exclusive writer.
// spinning_top v0.3 does not ship RwLock; spin::RwLock is the idiomatic choice.

pub struct RwLock<T>(spin::RwLock<T>);

impl<T> RwLock<T> {
    pub const fn new(val: T) -> Self {
        RwLock(spin::RwLock::new(val))
    }

    pub fn read(&self) -> spin::RwLockReadGuard<'_, T> {
        self.0.read()
    }

    pub fn write(&self) -> spin::RwLockWriteGuard<'_, T> {
        self.0.write()
    }

    pub fn try_read(&self) -> Option<spin::RwLockReadGuard<'_, T>> {
        self.0.try_read()
    }

    pub fn try_write(&self) -> Option<spin::RwLockWriteGuard<'_, T>> {
        self.0.try_write()
    }
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}
