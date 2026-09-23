//! Locks, backed by the standard library or by spin locks (`no_std`).

#[cfg(feature = "std")]
mod imp {
  use std::sync::{self, MutexGuard, PoisonError, RwLockReadGuard, RwLockWriteGuard};

  /// A mutual exclusion lock, ignoring poisoning.
  ///
  /// None of the guarded state can be left inconsistent by a panic.
  pub struct Mutex<T>(sync::Mutex<T>);

  impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
      Mutex(sync::Mutex::new(value))
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
      self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
  }

  /// A reader-writer lock, ignoring poisoning.
  pub struct RwLock<T>(sync::RwLock<T>);

  impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
      RwLock(sync::RwLock::new(value))
    }

    pub fn read(&self) -> RwLockReadGuard<'_, T> {
      self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
      self.0.write().unwrap_or_else(PoisonError::into_inner)
    }
  }
}

#[cfg(not(feature = "std"))]
mod imp {
  pub use spin::{Mutex, RwLock};
}

pub(crate) use self::imp::{Mutex, RwLock};
