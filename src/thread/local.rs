//! A thread-local pointer, stored in a slot of the operating system
//! (`pthread_getspecific` or `TlsGetValue`).
//!
//! Unlike `thread_local!`, accessing a slot does not resolve the storage of a
//! module, which may allocate (e.g. on the first access in a thread to a
//! variable of a library loaded with `dlopen`, on glibc).
//!
//! Only slots stored within the thread itself, for its entire lifetime, are
//! used, so getting and setting them never allocates nor fails, even whilst
//! the thread exits. Slots stored in memory allocated on first use, and freed
//! before the thread's last allocator calls, are rejected instead: those
//! with a key of 32 or above on glibc (`PTHREAD_KEY_2NDLEVEL_SIZE`), or an
//! index of 64 or above on Windows (`TLS_MINIMUM_AVAILABLE`). Other Unix-like
//! systems (e.g. FreeBSD) allocate all slots this way, so they are not
//! supported.

use crate::error::{Error, OsError, Result};
use core::sync::atomic::{AtomicUsize, Ordering};

/// The slot shared by all users, plus one, or zero if not yet created.
static SLOT: AtomicUsize = AtomicUsize::new(0);

/// A slot holding a pointer per thread, which is null in new threads.
#[derive(Clone, Copy)]
pub(crate) struct Slot(usize);

impl Slot {
  /// Returns the shared slot, creating it if required.
  pub fn get_or_create() -> Result<Self> {
    if let Some(slot) = Self::get() {
      return Ok(slot);
    }

    let created = imp::create().map_err(Error::Thread)?;
    if !imp::is_inline(created) {
      imp::delete(created);
      return Err(Error::Thread(OsError::other("no thread-local slot is stored inline")));
    }

    match SLOT.compare_exchange(0, created + 1, Ordering::AcqRel, Ordering::Acquire) {
      Ok(_) => Ok(Slot(created)),
      Err(existing) => {
        // Another thread created the slot first
        imp::delete(created);
        Ok(Slot(existing - 1))
      },
    }
  }

  /// Returns the shared slot, if created.
  #[inline]
  pub fn get() -> Option<Self> {
    SLOT.load(Ordering::Acquire).checked_sub(1).map(Slot)
  }

  /// Returns the value of the current thread.
  #[inline]
  pub fn value(self) -> *const () {
    imp::value(self.0)
  }

  /// Sets the value of the current thread.
  #[inline]
  pub fn set_value(self, value: *const ()) {
    imp::set_value(self.0, value);
  }
}

#[cfg(unix)]
mod imp {
  use crate::error::OsError;

  pub fn create() -> Result<usize, OsError> {
    let mut key = 0;
    // SAFETY: The key is written upon success. Values have no destructor.
    match unsafe { libc::pthread_key_create(&mut key, None) } {
      0 => Ok(key as usize),
      code => Err(OsError::from_errno(code)),
    }
  }

  /// Returns whether the values of `key` are stored within each thread. On
  /// musl, bionic & Apple platforms, all of them are.
  pub fn is_inline(key: usize) -> bool {
    // Keys beyond the first block are stored in blocks allocated on first
    // use, which are freed whilst the thread exits (`__nptl_deallocate_tsd`)
    const KEY_2NDLEVEL_SIZE: usize = 32;
    !cfg!(all(target_os = "linux", target_env = "gnu")) || key < KEY_2NDLEVEL_SIZE
  }

  pub fn delete(key: usize) {
    // SAFETY: The key was created by `create`, and never used.
    unsafe { libc::pthread_key_delete(key as libc::pthread_key_t) };
  }

  pub fn value(key: usize) -> *const () {
    // SAFETY: The key is valid, since it is never deleted once shared.
    unsafe { libc::pthread_getspecific(key as libc::pthread_key_t) }.cast_const().cast()
  }

  pub fn set_value(key: usize, value: *const ()) {
    // SAFETY: See `value`. It cannot fail, since the key is valid, and its
    // values are stored inline (see `is_inline`).
    let result = unsafe { libc::pthread_setspecific(key as libc::pthread_key_t, value.cast()) };
    debug_assert_eq!(result, 0);
  }
}

#[cfg(windows)]
mod imp {
  use crate::error::OsError;
  use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
  use windows_sys::Win32::System::Threading::{
    TLS_OUT_OF_INDEXES, TlsAlloc, TlsFree, TlsGetValue, TlsSetValue,
  };

  pub fn create() -> Result<usize, OsError> {
    // SAFETY: Allocates a TLS index, without preconditions.
    match unsafe { TlsAlloc() } {
      TLS_OUT_OF_INDEXES => Err(OsError::last_os_error()),
      index => Ok(index as usize),
    }
  }

  /// Returns whether the values of `index` are stored within each thread
  /// (the TEB). Others are stored in expansion slots, allocated on first use.
  pub fn is_inline(index: usize) -> bool {
    // `TLS_MINIMUM_AVAILABLE`, the number of slots in the TEB
    const TLS_MINIMUM_AVAILABLE: usize = 64;
    index < TLS_MINIMUM_AVAILABLE
  }

  pub fn delete(index: usize) {
    // SAFETY: The index was allocated by `create`, and never used.
    unsafe { TlsFree(index as u32) };
  }

  pub fn value(index: usize) -> *const () {
    // `TlsGetValue` clears the last error, which the target's caller may
    // rely upon
    // SAFETY: Reads the calling thread's last-error code.
    let error = unsafe { GetLastError() };
    // SAFETY: The index is valid, since it is never freed once shared.
    let value = unsafe { TlsGetValue(index as u32) };
    // SAFETY: Restores the calling thread's last-error code.
    unsafe { SetLastError(error) };
    value.cast_const().cast()
  }

  pub fn set_value(index: usize, value: *const ()) {
    // SAFETY: See `value`. It cannot fail, since the index is valid, and its
    // values are stored inline (see `is_inline`).
    let result = unsafe { TlsSetValue(index as u32, value.cast()) };
    debug_assert_ne!(result, 0);
  }
}
