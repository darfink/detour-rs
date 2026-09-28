//! Thread suspension, used to patch code whilst other threads are paused.
//!
//! Whilst threads are suspended, nothing may be allocated from the heap: a
//! suspended thread may hold the allocator's lock.

use crate::error::Result;

#[cfg(target_vendor = "apple")]
#[path = "apple.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;
#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "linux.rs"]
mod imp;
#[cfg(not(any(windows, target_vendor = "apple", target_os = "linux", target_os = "android")))]
#[path = "unsupported.rs"]
mod imp;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use self::imp::set_suspend_signal;

/// A thread of the current process.
///
/// Threads are identified by a platform-specific handle. Constructing a
/// `Thread` has no effect, so it is safe; its validity is instead required
/// by [`Transaction::commit`](crate::Transaction::commit), which is `unsafe`.
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

  /// Creates a thread from a POSIX thread (`pthread_t`).
  ///
  /// The thread must not exit (and be joined) whilst it is used.
  #[cfg(any(target_os = "linux", target_os = "android"))]
  pub fn from_pthread(thread: libc::pthread_t) -> Self {
    Thread(thread)
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
/// A suspended thread executing instructions that are patched is moved to
/// equivalent code (known as EIP relocation), so the transaction is safe
/// with regard to it.
///
/// On Linux and Android, threads are suspended by a real-time signal (see
/// `linux::set_suspend_signal`). Threads blocking the signal cannot be
/// suspended: `Only` fails, whilst `All` skips them (e.g. helper threads of
/// the C library, which block all signals). Skipped threads are not
/// protected, so they must not execute the patched instructions.
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
  session: imp::Session,
}

impl Suspended {
  /// Suspends `threads`.
  ///
  /// Once this returns, no heap allocations may be performed until the
  /// threads are resumed.
  pub fn new(threads: Threads<'_>) -> Result<Self> {
    let session = match threads {
      Threads::None => imp::Session::default(),
      Threads::All => imp::suspend_all()?,
      Threads::Only(threads) => imp::suspend(threads)?,
    };
    Ok(Suspended { session })
  }

  /// Returns the suspended threads.
  pub fn threads(&mut self) -> &mut [imp::Suspended] {
    &mut self.session
  }

  /// Applies all modified program counters.
  ///
  /// This may fail part way, but moving a thread is benign: it is only moved
  /// to equivalent code (of the original, or of a valid trampoline).
  pub fn apply(&mut self) -> Result<()> {
    self.session.iter_mut().try_for_each(imp::Suspended::apply)
  }
}

#[cfg(all(
  test,
  feature = "std",
  any(windows, target_vendor = "apple", target_os = "linux", target_os = "android")
))]
mod tests {
  use super::*;
  use core::arch::naked_asm;

  /// Spins forever.
  #[unsafe(naked)]
  extern "C" fn forever() -> i32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    naked_asm!("2:", "pause", "jmp 2b");
    #[cfg(target_arch = "aarch64")]
    naked_asm!("2:", "yield", "b 2b");
  }

  /// Returns `42`.
  #[unsafe(naked)]
  extern "C" fn escape() -> i32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    naked_asm!("mov eax, 42", "ret");
    #[cfg(target_arch = "aarch64")]
    naked_asm!("mov w0, #42", "ret");
  }

  #[test]
  fn moves_suspended_threads() -> Result<()> {
    let worker = std::thread::spawn(|| forever());
    let threads = [Thread::from(&worker)];
    let spin = forever as *const () as usize..forever as *const () as usize + 8;

    loop {
      let mut suspended = Suspended::new(Threads::Only(&threads))?;
      let [thread] = suspended.threads() else {
        panic!("the worker was not suspended")
      };
      if spin.contains(&thread.pc()) {
        thread.set_pc(escape as *const () as usize);
        suspended.apply()?;
        break;
      }
      // The worker has not yet entered `forever`
      drop(suspended);
      std::thread::yield_now();
    }

    assert_eq!(worker.join().unwrap(), 42);
    Ok(())
  }
}
