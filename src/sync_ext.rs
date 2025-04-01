use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait for `Mutex<T>` to provide `lock_unpoisoned`
pub trait MutexExt<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<T> {
        self.lock().unwrap_or_else(|err| {
            self.clear_poison();
            PoisonError::into_inner(err)
        })
    }
}

/// Extension trait for `RwLock<T>` to provide `read_unpoisoned` and `write_unpoisoned`
pub trait RwLockExt<T> {
    fn read_unpoisoned(&self) -> RwLockReadGuard<T>;
    fn write_unpoisoned(&self) -> RwLockWriteGuard<T>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    fn read_unpoisoned(&self) -> RwLockReadGuard<T> {
        self.read().unwrap_or_else(|err| {
            self.clear_poison();
            PoisonError::into_inner(err)
        })
    }

    fn write_unpoisoned(&self) -> RwLockWriteGuard<T> {
        self.write().unwrap_or_else(|err| {
            self.clear_poison();
            PoisonError::into_inner(err)
        })
    }
}
