//! Thread suspension, used to patch code whilst other threads are paused.
//!
//! Whilst threads are suspended, nothing may be allocated from the heap: a
//! suspended thread may hold the allocator's lock.

use crate::error::Result;
use alloc::vec::Vec;

#[cfg(target_vendor = "apple")]
#[path = "apple.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;
#[cfg(not(any(windows, target_vendor = "apple")))]
#[path = "unsupported.rs"]
mod imp;

/// A thread of the current process.
///
/// Threads are identified by a platform-specific handle, which must remain
/// valid (i.e. the thread must not be joined or detached and exit) until it
/// is no longer used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Thread(pub(crate) imp::RawThread);

impl Thread {
  /// Creates a thread from a raw thread handle (`HANDLE`).
  ///
  /// The handle requires the `THREAD_SUSPEND_RESUME`, `THREAD_GET_CONTEXT`,
  /// `THREAD_SET_CONTEXT` and `THREAD_QUERY_LIMITED_INFORMATION` access
  /// rights (e.g. `THREAD_ALL_ACCESS`).
  #[cfg(windows)]
  pub fn from_raw_handle(handle: *mut core::ffi::c_void) -> Self {
    Thread(handle as usize)
  }

  /// Creates a thread from a Mach thread port (`thread_act_t`).
  #[cfg(target_vendor = "apple")]
  pub fn from_mach_port(port: u32) -> Self {
    Thread(port)
  }
}

#[cfg(feature = "std")]
impl<T> From<&std::thread::JoinHandle<T>> for Thread {
  fn from(handle: &std::thread::JoinHandle<T>) -> Self {
    #[cfg(windows)]
    {
      use std::os::windows::io::AsRawHandle;
      Thread::from_raw_handle(handle.as_raw_handle())
    }
    #[cfg(unix)]
    {
      use std::os::unix::thread::JoinHandleExt;
      imp::from_pthread(handle.as_pthread_t())
    }
  }
}

/// The threads to suspend whilst a [`Transaction`](crate::Transaction) is
/// committed.
///
/// Suspended threads that are executing code that is patched are moved
/// accordingly (i.e. instruction pointer relocation), so the transaction is
/// safe with regard to them.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Threads<'a> {
  /// Suspend no threads.
  None,
  /// Suspend all other threads of the process.
  ///
  /// Threads created whilst the threads are enumerated are also suspended.
  All,
  /// Suspend the given threads. The current thread is ignored.
  Only(&'a [Thread]),
}

/// A set of suspended threads, which are resumed once dropped.
pub(crate) struct Suspended {
  threads: Vec<imp::Suspended>,
}

impl Suspended {
  /// Suspends `threads`.
  ///
  /// Once this returns, no heap allocations may be performed until the
  /// threads are resumed.
  pub fn new(threads: Threads<'_>) -> Result<Self> {
    let threads = match threads {
      Threads::None => Vec::new(),
      Threads::All => imp::suspend_all()?,
      Threads::Only(threads) => imp::suspend(threads)?,
    };
    Ok(Suspended { threads })
  }

  /// Returns the suspended threads.
  pub fn threads(&mut self) -> &mut [imp::Suspended] {
    &mut self.threads
  }

  /// Applies all modified program counters.
  ///
  /// This may fail part way, but moving a thread is benign: it is only moved
  /// to equivalent code (of the original, or of a valid trampoline).
  pub fn apply(&mut self) -> Result<()> {
    self.threads.iter_mut().try_for_each(imp::Suspended::apply)
  }
}
