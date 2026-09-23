//! A lock, backed by the standard library or by a spin lock (`no_std`).

#[cfg(feature = "std")]
mod imp {
  use std::sync::{self, MutexGuard, PoisonError};

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
}

#[cfg(not(feature = "std"))]
mod imp {
  pub use spin::Mutex;
}

pub(crate) use self::imp::Mutex;
