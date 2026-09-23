use crate::error::{Error, Result};
use crate::sync::Mutex;
use crate::{Function, TypedDetour};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use core::{mem, ptr};

/// A type-safe static detour.
///
/// To define a static detour, use the [`static_detour`](crate::static_detour)
/// macro. Since its detour is a closure, it is created using
/// [`initialize`](#method.initialize) instead of a constructor.
///
/// # Example
///
/// ```rust
/// use std::error::Error;
/// use detour::static_detour;
///
/// static_detour! {
///   static Test: fn(i32) -> i32;
/// }
///
/// #[inline(never)]
/// fn add5(val: i32) -> i32 {
///   val + 5
/// }
///
/// fn add10(val: i32) -> i32 {
///   val + 10
/// }
///
/// fn main() -> Result<(), Box<dyn Error>> {
///   // Replace the 'add5' function with 'add10' (can also be a closure)
///   unsafe { Test.initialize(add5, add10)? };
///
///   assert_eq!(add5(1), 6);
///   assert_eq!(Test.call(1), 6);
///
///   unsafe { Test.enable()? };
///
///   // The original function is detoured to 'add10'
///   assert_eq!(add5(1), 11);
///
///   // The original function can still be invoked using 'call'
///   assert_eq!(Test.call(1), 6);
///
///   // It is also possible to change the detour whilst hooked
///   Test.set_detour(|val| val - 5);
///   assert_eq!(add5(5), 0);
///
///   unsafe { Test.disable()? };
///
///   assert_eq!(add5(1), 6);
///   Ok(())
/// }
/// ```
pub struct StaticDetour<T: Function> {
  // The active closure, or null if not yet initialized.
  closure: AtomicPtr<Box<T::Closure>>,
  // The number of calls currently executing a closure.
  active_calls: AtomicUsize,
  // Replaced closures, which may still be executing.
  retired: Mutex<Vec<Box<Box<T::Closure>>>>,
  // Set once, and never released (statics are never dropped)
  detour: AtomicPtr<TypedDetour<T>>,
  ffi: T,
}

impl<T: Function> StaticDetour<T> {
  /// Create a new static detour.
  #[doc(hidden)]
  pub const fn __new(ffi: T) -> Self {
    StaticDetour {
      closure: AtomicPtr::new(ptr::null_mut()),
      active_calls: AtomicUsize::new(0),
      retired: Mutex::new(Vec::new()),
      detour: AtomicPtr::new(ptr::null_mut()),
      ffi,
    }
  }

  /// Creates the detour; see `initialize`.
  pub(crate) unsafe fn initialize_shared(
    &self,
    target: T,
    closure: Box<T::Closure>,
  ) -> Result<&Self> {
    if self.get().is_some() {
      return Err(Error::AlreadyInitialized);
    }

    // SAFETY: The target is compatible with the generated FFI function.
    let detour = Box::into_raw(Box::new(unsafe { TypedDetour::new(target, self.ffi)? }));

    // Another thread may have raced to initialize the detour
    let result =
      self
        .detour
        .compare_exchange(ptr::null_mut(), detour, Ordering::AcqRel, Ordering::Acquire);
    if result.is_err() {
      // SAFETY: The detour was never shared.
      drop(unsafe { Box::from_raw(detour) });
      return Err(Error::AlreadyInitialized);
    }
    self.set_detour_shared(closure);
    Ok(self)
  }

  /// Enables the detour.
  ///
  /// # Safety
  ///
  /// See [`TypedDetour::enable`].
  pub unsafe fn enable(&self) -> Result<()> {
    let detour = self.get().ok_or(Error::NotInitialized)?;
    // SAFETY: Forwarded from the caller.
    unsafe { detour.enable() }
  }

  /// Disables the detour.
  ///
  /// # Safety
  ///
  /// See [`TypedDetour::enable`].
  pub unsafe fn disable(&self) -> Result<()> {
    let detour = self.get().ok_or(Error::NotInitialized)?;
    // SAFETY: Forwarded from the caller.
    unsafe { detour.disable() }
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.get().is_some_and(TypedDetour::is_enabled)
  }

  /// Replaces the detour closure; see `set_detour`.
  ///
  /// The previous closure may still be executing on another thread (or be the
  /// caller), so it is retired and released once no call is in progress.
  ///
  /// All operations on `closure` and `active_calls` are sequentially
  /// consistent. A call increments `active_calls` before loading `closure`,
  /// so if `active_calls` is observed as zero after a closure was swapped out,
  /// every call that may have loaded it has completed.
  pub(crate) fn set_detour_shared(&self, closure: Box<T::Closure>) {
    let previous = self.closure.swap(Box::into_raw(Box::new(closure)), Ordering::SeqCst);

    let released = {
      let mut retired = self.retired.lock();
      if !previous.is_null() {
        // SAFETY: The pointer originates from `Box::into_raw`, and was
        // exclusively obtained by the swap.
        retired.push(unsafe { Box::from_raw(previous) });
      }

      if self.active_calls.load(Ordering::SeqCst) == 0 {
        mem::take(&mut *retired)
      } else {
        Vec::new()
      }
    };

    // Released outside of the lock, in case a closure's destructor calls the
    // detoured function.
    drop(released);
  }

  /// Invokes `call` with the active detour closure.
  ///
  /// Panics if the static detour has not yet been initialized.
  #[doc(hidden)]
  #[inline]
  pub fn __with_detour<R>(&self, call: impl FnOnce(&T::Closure) -> R) -> R {
    struct Active<'a>(&'a AtomicUsize);

    impl Drop for Active<'_> {
      fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
      }
    }

    self.active_calls.fetch_add(1, Ordering::SeqCst);
    let _active = Active(&self.active_calls);

    // SAFETY: The closure is only released once no call is active (see
    // `set_detour_shared`), or when `self` is dropped.
    let closure = unsafe { self.closure.load(Ordering::SeqCst).as_ref() }
      .expect("static detour closure is not initialized");
    call(closure)
  }

  /// Returns the detour, if initialized.
  fn get(&self) -> Option<&TypedDetour<T>> {
    // SAFETY: The pointer is either null, or a detour that is never released
    // whilst `self` is alive.
    unsafe { self.detour.load(Ordering::Acquire).as_ref() }
  }

  /// Returns a pointer to the generated trampoline.
  pub(crate) fn trampoline_ptr(&self) -> *const () {
    self
      .get()
      .expect("static detour is not initialized")
      .trampoline_ptr()
  }
}

impl<T: Function> Drop for StaticDetour<T> {
  fn drop(&mut self) {
    // The detour is disabled first, so no closure can be executing.
    let detour = *self.detour.get_mut();
    if !detour.is_null() {
      // SAFETY: The detour is exclusively owned by `self`.
      drop(unsafe { Box::from_raw(detour) });
    }

    let closure = *self.closure.get_mut();
    if !closure.is_null() {
      // SAFETY: The closure is exclusively owned by `self`.
      drop(unsafe { Box::from_raw(closure) });
    }
  }
}

impl<T: Function> core::fmt::Debug for StaticDetour<T> {
  fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    f.debug_struct("StaticDetour")
      .field("detour", &self.get())
      .finish_non_exhaustive()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::AtomicBool;
  use std::thread;

  type Closure = dyn Fn(u32) -> u32 + Send + Sync;

  fn original(value: u32) -> u32 {
    value
  }

  /// Counts the number of released closures.
  struct Tracked(Arc<AtomicUsize>);

  impl Drop for Tracked {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  fn tracked(value: u32, released: &Arc<AtomicUsize>) -> Box<Closure> {
    let token = Tracked(released.clone());
    Box::new(move |x| {
      let _ = &token;
      x + value
    })
  }

  #[test]
  fn replaced_closures_are_released_when_inactive() {
    let detour = StaticDetour::<fn(u32) -> u32>::__new(original);
    let released = Arc::new(AtomicUsize::new(0));

    detour.set_detour_shared(tracked(1, &released));
    assert_eq!(detour.__with_detour(|closure| closure(1)), 2);

    detour.set_detour_shared(tracked(2, &released));
    assert_eq!(released.load(Ordering::SeqCst), 1);

    // A closure replaced whilst executing remains valid until it returns
    detour.__with_detour(|closure| {
      detour.set_detour_shared(tracked(3, &released));
      assert_eq!(closure(1), 3);
    });
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(detour.__with_detour(|closure| closure(1)), 4);

    // Retired closures are released upon the next inactive replacement
    detour.set_detour_shared(tracked(4, &released));
    assert_eq!(released.load(Ordering::SeqCst), 3);

    drop(detour);
    assert_eq!(released.load(Ordering::SeqCst), 4);
  }

  #[test]
  fn concurrent_replacement() {
    const REPLACEMENTS: u32 = 1000;

    let detour = StaticDetour::<fn(u32) -> u32>::__new(original);
    let released = Arc::new(AtomicUsize::new(0));
    let done = AtomicBool::new(false);
    detour.set_detour_shared(tracked(0, &released));

    thread::scope(|scope| {
      for _ in 0..4 {
        scope.spawn(|| {
          while !done.load(Ordering::Relaxed) {
            assert!(detour.__with_detour(|closure| closure(0)) < REPLACEMENTS);
          }
        });
      }

      for value in 1..REPLACEMENTS {
        detour.set_detour_shared(tracked(value, &released));
      }
      done.store(true, Ordering::Relaxed);
    });

    // All closures but the active one are released once no call is active
    detour.set_detour_shared(tracked(0, &released));
    assert_eq!(released.load(Ordering::SeqCst), REPLACEMENTS as usize);
  }
}
