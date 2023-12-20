use std::{
    cell::RefCell,
    ops,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// A trait represents a shared value that can be dynamically checked for borrowing rules
/// or protected by mutual exclusion.
pub trait Shared {
    /// The real type of this shared value.
    type Value;

    /// The immutable reference type of this shared value.
    type Ref<'a>: ops::Deref<Target = Self::Value>
    where
        Self: 'a;

    /// The mutable reference type of this shared value.
    type MutRef<'a>: ops::DerefMut<Target = Self::Value>
    where
        Self: 'a;

    /// Lock shared value and get immutable reference.
    fn lock(&self) -> Self::Ref<'_>;

    /// Lock shared value and get mutable reference.
    fn lock_mut(&self) -> Self::MutRef<'_>;

    /// Try lock shared value and get mutable reference.
    ///
    /// If the lock is not successful, returns [`None`]
    fn try_lock_mut(&self) -> Option<Self::MutRef<'_>>;
}

/// Shared data that using in single thread mode
#[derive(Debug, Clone)]
pub struct LocalShared<T> {
    value: Rc<RefCell<T>>,
}

impl<T> Shared for LocalShared<T> {
    type Value = T;

    type Ref<'a> = std::cell::Ref<'a,T>
    where
        Self: 'a;

    type MutRef<'a> = std::cell::RefMut<'a,T>
    where
        Self: 'a;

    fn lock(&self) -> Self::Ref<'_> {
        self.value.borrow()
    }

    fn lock_mut(&self) -> Self::MutRef<'_> {
        self.value.borrow_mut()
    }

    fn try_lock_mut(&self) -> Option<Self::MutRef<'_>> {
        match self.value.try_borrow_mut() {
            Ok(value) => Some(value),
            // the value is currently borrowed
            _ => None,
        }
    }
}

impl<T> LocalShared<T> {
    /// Create new `LocalShared` from shared `value`.
    pub fn new(value: T) -> Self {
        value.into()
    }
}

impl<T> From<T> for LocalShared<T> {
    fn from(value: T) -> Self {
        LocalShared {
            value: Rc::new(RefCell::new(value)),
        }
    }
}

/// Shared data that using in multi-thread mode
#[derive(Debug, Clone)]
pub struct MutexShared<T> {
    value: Arc<Mutex<T>>,
}

impl<T> Shared for MutexShared<T> {
    type Value = T;

    type Ref<'a> = std::sync::MutexGuard<'a,T>
    where
        Self: 'a;

    type MutRef<'a> = std::sync::MutexGuard<'a,T>
    where
        Self: 'a;

    fn lock(&self) -> Self::Ref<'_> {
        self.value.lock().unwrap()
    }

    fn lock_mut(&self) -> Self::MutRef<'_> {
        self.value.lock().unwrap()
    }

    fn try_lock_mut(&self) -> Option<Self::MutRef<'_>> {
        match self.value.try_lock() {
            Ok(value) => Some(value),
            // the value is currently borrowed
            _ => None,
        }
    }
}

impl<T> From<T> for MutexShared<T> {
    fn from(value: T) -> Self {
        MutexShared {
            value: Arc::new(Mutex::new(value)),
        }
    }
}

impl<T> MutexShared<T> {
    /// Create new `MutexShared` from shared `value`.
    pub fn new(value: T) -> Self {
        value.into()
    }
}