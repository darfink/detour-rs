use error::*;
use std::{mem, ptr};
use std::sync::atomic::{AtomicPtr, Ordering};
use {GenericDetour, Function};

/// A type-safe static detour.
pub struct StaticDetour<T: Function> {
  closure: AtomicPtr<Box<Fn<T::Arguments, Output = T::Output>>>,
  detour: AtomicPtr<GenericDetour<T>>,
  ffi: T,
}

impl<T: Function> StaticDetour<T> {
  /// Create a new static detour.
  #[doc(hidden)]
  pub const fn __new(ffi: T) -> Self {
    StaticDetour {
      closure: AtomicPtr::new(ptr::null_mut()),
      detour: AtomicPtr::new(ptr::null_mut()),
      ffi,
    }
  }

  /// Create a new hook given a target function and a compatible detour
  /// closure.
  pub unsafe fn initialize<D>(&self, target: T, closure: D) -> Result<()>
  where
    D: Fn<T::Arguments, Output = T::Output> + Send + 'static
  {
    let mut detour = Box::new(GenericDetour::new(target, self.ffi)?);
    if !self.detour.compare_and_swap(ptr::null_mut(), &mut *detour, Ordering::SeqCst).is_null() {
      Err(Error::AlreadyInitialized)?;
    }

    self.set_detour(closure);
    mem::forget(detour);
    Ok(())
  }

  /// Enables the detour.
  pub unsafe fn enable(&self) -> Result<()> {
    self.detour.load(Ordering::SeqCst).as_ref().ok_or(Error::NotInitialized)?.enable()
  }

  /// Disables the detour.
  pub unsafe fn disable(&self) -> Result<()> {
    self.detour.load(Ordering::SeqCst).as_ref().ok_or(Error::NotInitialized)?.disable()
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    unsafe { self.detour.load(Ordering::SeqCst).as_ref() }.map(|detour| detour.is_enabled()).unwrap_or(false)
  }

  /// Changes the detour, regardless of whether the target is hooked or not.
  pub fn set_detour<C>(&self, closure: C)
  where
    C: Fn<T::Arguments, Output = T::Output> + Send + 'static,
  {
    let previous = self.closure.swap(Box::into_raw(Box::new(Box::new(closure))), Ordering::SeqCst);
    if !previous.is_null() {
      unsafe { Box::from_raw(previous) };
    }
  }

  /// Returns a reference to the generated trampoline.
  pub(crate) fn trampoline(&self) -> Result<&()> {
    Ok(unsafe { self.detour.load(Ordering::SeqCst).as_ref() }.ok_or(Error::NotInitialized)?.trampoline())
  }

  /// Returns a transient reference to the active detour.
  #[doc(hidden)]
  pub fn __detour(&self) -> &Box<Fn<T::Arguments, Output = T::Output>> {
    // TODO: This is not 100% thread-safe in case the thread is stopped
    unsafe { self.closure.load(Ordering::SeqCst).as_ref() }.ok_or(Error::NotInitialized).unwrap()
  }
}

impl<T: Function> Drop for StaticDetour<T> {
  fn drop(&mut self) {
    let previous = self.closure.swap(::std::ptr::null_mut(), Ordering::Relaxed);
    if !previous.is_null() {
      unsafe { Box::from_raw(previous) };
    }

    let previous = self.detour.swap(::std::ptr::null_mut(), Ordering::Relaxed);
    if !previous.is_null() {
      unsafe { Box::from_raw(previous) };
    }
  }
}
