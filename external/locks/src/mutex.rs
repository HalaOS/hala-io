use std::{collections::VecDeque, ops, task::Waker};

use crate::{Locker, LockerGuard, SpinMutex, WaitableLockerMaker};

impl<T> Locker for std::sync::Mutex<T> {
    type Value = T;

    type Guard<'a> = MutexGuard<'a, T>
    where
        Self: 'a,
        Self::Value: 'a;

    fn sync_lock(&self) -> Self::Guard<'_> {
        MutexGuard {
            std_guard: Some(self.lock().unwrap()),
        }
    }

    fn try_sync_lock(&self) -> Option<Self::Guard<'_>> {
        match self.try_lock() {
            Ok(guard) => Some(MutexGuard {
                std_guard: Some(guard),
            }),
            _ => None,
        }
    }

    fn new(data: Self::Value) -> Self {
        std::sync::Mutex::new(data)
    }
}

#[derive(Debug)]
pub struct MutexGuard<'a, T> {
    std_guard: Option<std::sync::MutexGuard<'a, T>>,
}

impl<'a, T> ops::Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.std_guard.as_deref().unwrap()
    }
}

impl<'a, T> ops::DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.std_guard.as_deref_mut().unwrap()
    }
}

impl<'a, T> LockerGuard<'a> for MutexGuard<'a, T> {
    fn unlock(&mut self) {
        self.std_guard.take();
    }
}
