//! A lock, backed by the standard library or by a spin lock (without `std`).

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
  use core::cell::UnsafeCell;
  use core::ops::{Deref, DerefMut};
  use core::sync::atomic::{AtomicBool, Ordering};

  /// A minimal spin lock.
  ///
  /// Locks are held briefly (whilst code is patched or allocated), so
  /// spinning is acceptable.
  pub struct Mutex<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
  }

  // SAFETY: Access to the value is serialized by the lock.
  unsafe impl<T: Send> Send for Mutex<T> {}
  // SAFETY: See above.
  unsafe impl<T: Send> Sync for Mutex<T> {}

  impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
      Mutex {
        locked: AtomicBool::new(false),
        value: UnsafeCell::new(value),
      }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
      while self
        .locked
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
      {
        while self.locked.load(Ordering::Relaxed) {
          core::hint::spin_loop();
        }
      }
      MutexGuard(self)
    }
  }

  /// Releases the lock once dropped.
  pub struct MutexGuard<'a, T>(&'a Mutex<T>);

  impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
      // SAFETY: The lock is held.
      unsafe { &*self.0.value.get() }
    }
  }

  impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
      // SAFETY: The lock is held exclusively.
      unsafe { &mut *self.0.value.get() }
    }
  }

  impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
      self.0.locked.store(false, Ordering::Release);
    }
  }
}

pub(crate) use self::imp::Mutex;
