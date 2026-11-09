//! Reentrancy of static detours (see the generated `set_reentrant`).
//!
//! Each thread has a stack of frames, one per non-reentrant detour whose
//! closure is executing, linked from the innermost (stored in a thread-local
//! slot, see `thread::local`). Frames are allocated on the stack, since a
//! detour of an allocation function must not allocate.

use crate::error::Result;
use core::sync::atomic::{AtomicBool, Ordering};

/// Whether a detour's closure may be invoked whilst it is executing on the
/// same thread.
pub(crate) struct Reentrancy {
  reentrant: AtomicBool,
}

impl Reentrancy {
  /// Creates a reentrant state.
  pub const fn new() -> Self {
    Reentrancy {
      reentrant: AtomicBool::new(true),
    }
  }

  /// Sets whether the detour is reentrant.
  ///
  /// Fails if executing closures cannot be tracked, i.e. if no thread-local
  /// slot is available, or on unsupported platforms.
  pub fn set(&self, reentrant: bool) -> Result<()> {
    if !reentrant {
      imp::prepare()?;
    }
    // Releases the prepared slot to calls observing the detour as
    // non-reentrant
    self.reentrant.store(reentrant, Ordering::Release);
    Ok(())
  }

  /// Invokes `call` with whether `detour` is non-reentrant and already
  /// executing on this thread. If not, it is executing until `call` returns.
  #[inline]
  pub fn enter<R>(&self, detour: *const (), call: impl FnOnce(bool) -> R) -> R {
    if self.reentrant.load(Ordering::Acquire) {
      call(false)
    } else {
      imp::enter(detour, call)
    }
  }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows))]
mod imp {
  use crate::error::Result;
  use crate::thread::local::Slot;

  struct Frame {
    detour: *const (),
    outer: *const Frame,
  }

  pub fn prepare() -> Result<()> {
    Slot::get_or_create().map(drop)
  }

  pub fn enter<R>(detour: *const (), call: impl FnOnce(bool) -> R) -> R {
    struct Exit(Slot, *const Frame);

    impl Drop for Exit {
      fn drop(&mut self) {
        self.0.set_value(self.1.cast());
      }
    }

    // The slot has been prepared before the detour became non-reentrant
    let Some(slot) = Slot::get() else {
      return call(false);
    };

    let outer = slot.value().cast::<Frame>();
    let mut frame = outer;
    // SAFETY: Frames are only linked whilst they are alive (see `Exit`).
    while let Some(current) = unsafe { frame.as_ref() } {
      if current.detour == detour {
        return call(true);
      }
      frame = current.outer;
    }

    let frame = Frame { detour, outer };
    slot.set_value((&raw const frame).cast());
    // Dropped before `frame`, also when unwinding
    let _exit = Exit(slot, outer);
    call(false)
  }
}

/// Platforms without a suitable thread-local slot, on which detours are
/// always reentrant.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
mod imp {
  use crate::error::{Error, Result};

  pub fn prepare() -> Result<()> {
    Err(Error::ThreadsUnsupported)
  }

  pub fn enter<R>(_detour: *const (), call: impl FnOnce(bool) -> R) -> R {
    call(false)
  }
}
