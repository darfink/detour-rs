//! The architecture-independent implementation of a detour.

use crate::arch;
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock};
use alloc::boxed::Box;
use core::fmt;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicBool, Ordering};

/// The building block of all detour types.
pub(crate) struct Detour {
  target: *const (),
  detour: *const (),
  patch_address: *mut u8,
  original: Box<[u8]>,
  patched: Box<[u8]>,
  // Released manually, since the target may still refer to them
  trampoline: ManuallyDrop<CodeBlock>,
  relay: ManuallyDrop<Option<CodeBlock>>,
  enabled: AtomicBool,
}

// SAFETY: The raw pointers refer to code, which is not tied to a thread, and
// all modifications of it are serialized by the global patch lock.
unsafe impl Send for Detour {}
// SAFETY: See above; `&Detour` only exposes atomic state and locked operations.
unsafe impl Sync for Detour {}

impl Detour {
  /// Creates a new, disabled detour.
  ///
  /// # Safety
  ///
  /// `target` must point to a function, and `detour` to a function compatible
  /// with the target's calling convention and signature.
  pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
    if target == detour {
      return Err(Error::SameAddress);
    }

    let _lock = memory::patch_lock();

    if !memory::is_executable(target)? || !memory::is_executable(detour)? {
      return Err(Error::NotExecutable);
    }

    // SAFETY: The target has been verified to be executable.
    let hook = unsafe { arch::build(target, detour)? };

    Ok(Detour {
      target,
      detour,
      patch_address: hook.patch_address,
      original: hook.original.into_boxed_slice(),
      patched: hook.patched.into_boxed_slice(),
      trampoline: ManuallyDrop::new(hook.trampoline),
      relay: ManuallyDrop::new(hook.relay),
      enabled: AtomicBool::new(false),
    })
  }

  /// Enables the detour.
  pub unsafe fn enable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.toggle(true) }
  }

  /// Disables the detour.
  pub unsafe fn disable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.toggle(false) }
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.enabled.load(Ordering::SeqCst)
  }

  /// Returns a pointer to the trampoline (i.e. the callable original).
  pub fn trampoline(&self) -> *const () {
    self.trampoline.as_ptr().cast()
  }

  /// Enables or disables the detour.
  unsafe fn toggle(&self, enable: bool) -> Result<()> {
    let _lock = memory::patch_lock();

    if self.enabled.load(Ordering::SeqCst) == enable {
      return Ok(());
    }

    let (expected, replacement) = if enable {
      (&self.original, &self.patched)
    } else {
      (&self.patched, &self.original)
    };

    // Refuse to overwrite code that has been modified by someone else, e.g.
    // another detour of the same target that was enabled after this one.
    // SAFETY: The patch area is readable (it was read upon creation).
    let current = unsafe { core::slice::from_raw_parts(self.patch_address, expected.len()) };
    if current != &expected[..] {
      return Err(Error::TargetModified);
    }

    // SAFETY: The replacement is either the original code, or a branch to a
    // detour that has been verified during creation.
    unsafe { memory::patch_code(self.patch_address, replacement)? };
    self.enabled.store(enable, Ordering::SeqCst);
    Ok(())
  }
}

impl Drop for Detour {
  /// Disables the detour, if enabled.
  fn drop(&mut self) {
    // SAFETY: Restoring the original code is always valid.
    if unsafe { self.disable() }.is_ok() {
      // SAFETY: The target no longer refers to the generated code, and the
      // fields are never accessed again.
      unsafe {
        ManuallyDrop::drop(&mut self.trampoline);
        ManuallyDrop::drop(&mut self.relay);
      }
    }
    // Otherwise the target may still branch to the trampoline or relay, so
    // their memory is intentionally leaked.
  }
}

impl fmt::Debug for Detour {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Detour")
      .field("target", &self.target)
      .field("detour", &self.detour)
      .field("trampoline", &self.trampoline())
      .field("enabled", &self.is_enabled())
      .finish()
  }
}
