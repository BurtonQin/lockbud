use std::sync::{RwLock, RwLockWriteGuard, RwLockReadGuard};

pub trait HandyRwLock<T> {
    fn wl(&self) -> RwLockWriteGuard<'_, T>;
    fn rl(&self) -> RwLockReadGuard<'_, T>;
}

impl<T> HandyRwLock<T> for RwLock<T> {
    fn wl(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap()
    }

    fn rl(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap()
    }
}
