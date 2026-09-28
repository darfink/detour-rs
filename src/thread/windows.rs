//! Thread suspension using the Win32 API.

use super::Thread;
use crate::error::{Error, OsError, Result};
use alloc::vec::Vec;
use core::mem;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext, SetThreadContext};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
  CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
  GetCurrentProcessId, GetCurrentThreadId, GetThreadId, OpenThread, ResumeThread, SuspendThread,
  THREAD_GET_CONTEXT, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
};

#[cfg(target_arch = "aarch64")]
use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_CONTROL_ARM64 as CONTEXT_CONTROL;
#[cfg(target_arch = "x86_64")]
use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_CONTROL_AMD64 as CONTEXT_CONTROL;
#[cfg(target_arch = "x86")]
use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_CONTROL_X86 as CONTEXT_CONTROL;

/// A `HANDLE`, stored as an integer so threads are `Send` & `Sync`.
pub(crate) type RawThread = usize;

/// The suspended threads, resumed once dropped.
pub(crate) type Session = Vec<Suspended>;

/// A thread context, which must be 16-byte aligned on x86-64.
#[repr(C, align(16))]
struct Context(CONTEXT);

fn last_error() -> Error {
  Error::Thread(OsError::last_os_error())
}

/// A suspended thread, which is resumed once dropped.
pub(crate) struct Suspended {
  handle: HANDLE,
  id: u32,
  /// Whether the handle is owned (and must be closed).
  owned: bool,
  context: Context,
  modified: bool,
}

impl Suspended {
  /// Suspends the thread of `handle`, closing the handle on failure if owned.
  fn new(handle: HANDLE, id: u32, owned: bool) -> Result<Self> {
    let fail = |error| {
      if owned {
        // SAFETY: The handle is owned.
        unsafe { CloseHandle(handle) };
      }
      Err(error)
    };

    // SAFETY: Suspending a thread has no memory safety implications.
    if unsafe { SuspendThread(handle) } == u32::MAX {
      return fail(last_error());
    }

    // Suspension is asynchronous; retrieving the context waits for it.
    // SAFETY: A zeroed context is valid.
    let mut context: Context = unsafe { mem::zeroed() };
    context.0.ContextFlags = CONTEXT_CONTROL;
    // SAFETY: The context is aligned, and the thread is suspended.
    if unsafe { GetThreadContext(handle, &mut context.0) } == 0 {
      let error = last_error();
      // SAFETY: The thread was suspended above.
      unsafe { ResumeThread(handle) };
      return fail(error);
    }

    Ok(Suspended {
      handle,
      id,
      owned,
      context,
      modified: false,
    })
  }

  /// Returns the program counter.
  pub fn pc(&self) -> usize {
    #[cfg(target_arch = "aarch64")]
    return self.context.0.Pc as usize;
    #[cfg(target_arch = "x86_64")]
    return self.context.0.Rip as usize;
    #[cfg(target_arch = "x86")]
    return self.context.0.Eip as usize;
  }

  /// Sets the program counter, which is applied by [`Suspended::apply`].
  pub fn set_pc(&mut self, pc: usize) {
    #[cfg(target_arch = "aarch64")]
    {
      self.context.0.Pc = pc as u64;
    }
    #[cfg(target_arch = "x86_64")]
    {
      self.context.0.Rip = pc as u64;
    }
    #[cfg(target_arch = "x86")]
    {
      self.context.0.Eip = pc as u32;
    }
    self.modified = true;
  }

  /// Applies a modified program counter.
  pub(super) fn apply(&mut self) -> Result<()> {
    if !mem::take(&mut self.modified) {
      return Ok(());
    }
    // SAFETY: The context was retrieved from the (suspended) thread.
    if unsafe { SetThreadContext(self.handle, &self.context.0) } == 0 {
      return Err(last_error());
    }
    Ok(())
  }
}

impl Drop for Suspended {
  fn drop(&mut self) {
    // SAFETY: The thread was suspended by `self`, and the handle is closed
    // only if owned.
    unsafe {
      ResumeThread(self.handle);
      if self.owned {
        CloseHandle(self.handle);
      }
    }
  }
}

/// Suspends all other threads of the process.
///
/// The threads are enumerated repeatedly until no new threads appear. A
/// snapshot is allocated using virtual memory (not the process heap), so
/// this is safe whilst threads are suspended.
pub(super) fn suspend_all() -> Result<Session> {
  const RIGHTS: u32 = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT;

  // SAFETY: These have no preconditions.
  let (process, current) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
  let mut suspended: Vec<Suspended> = Vec::new();

  loop {
    // SAFETY: Creates a snapshot of all threads in the system.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
      return Err(last_error());
    }

    // SAFETY: A zeroed entry is valid.
    let mut entry: THREADENTRY32 = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;

    let (mut added, mut complete, mut count) = (false, true, 0);
    // SAFETY: The snapshot is valid, and the entry's size is set.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;

    while more {
      let id = entry.th32ThreadID;
      if entry.th32OwnerProcessID == process && id != current {
        count += 1;
        if suspended.iter().all(|thread| thread.id != id) {
          if suspended.len() == suspended.capacity() {
            complete = false;
          } else {
            // SAFETY: Opening a thread has no memory safety implications.
            let handle = unsafe { OpenThread(RIGHTS, 0, id) };
            // A thread may exit whilst being enumerated; it is ignored
            if !handle.is_null() {
              if let Ok(thread) = Suspended::new(handle, id, true) {
                suspended.push(thread);
                added = true;
              }
            }
          }
        }
      }
      // SAFETY: See above.
      more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }

    // SAFETY: The snapshot is owned.
    unsafe { CloseHandle(snapshot) };

    if !complete {
      // Resume all threads before allocating, and retry with a larger capacity
      drop(mem::take(&mut suspended));
      suspended = Vec::with_capacity(count * 2 + 16);
    } else if !added {
      return Ok(suspended);
    }
  }
}

/// Suspends the given threads, ignoring the current thread.
pub(super) fn suspend(threads: &[Thread]) -> Result<Session> {
  // SAFETY: This has no preconditions.
  let current = unsafe { GetCurrentThreadId() };
  let mut suspended = Vec::with_capacity(threads.len());

  for thread in threads {
    let handle = thread.0 as HANDLE;
    // SAFETY: Querying an invalid handle fails gracefully.
    let id = unsafe { GetThreadId(handle) };
    if id == 0 {
      return Err(last_error());
    }
    if id != current {
      suspended.push(Suspended::new(handle, id, false)?);
    }
  }
  Ok(suspended)
}

