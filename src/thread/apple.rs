//! Thread suspension using Mach thread primitives.

use super::Thread;
use crate::error::{Error, OsError, Result};
use alloc::vec::Vec;
use core::mem;
use mach2::kern_return::{KERN_SUCCESS, kern_return_t};
use mach2::mach_init::mach_thread_self;
use mach2::mach_port::mach_port_deallocate;
use mach2::mach_types::{thread_act_array_t, thread_act_t};
use mach2::message::mach_msg_type_number_t;
use mach2::task::task_threads;
use mach2::thread_act::{thread_get_state, thread_resume, thread_set_state, thread_suspend};
use mach2::thread_status::thread_state_t;
use mach2::traps::mach_task_self;
use mach2::vm::mach_vm_deallocate;

#[cfg(target_arch = "aarch64")]
use mach2::{structs::arm_thread_state64_t as State, thread_status::ARM_THREAD_STATE64 as FLAVOR};
#[cfg(target_arch = "x86_64")]
use mach2::{structs::x86_thread_state64_t as State, thread_status::x86_THREAD_STATE64 as FLAVOR};

pub(crate) type RawThread = thread_act_t;

/// The suspended threads, resumed once dropped.
pub(crate) type Session = Vec<Suspended>;

#[cfg(feature = "std")]
pub(super) fn from_pthread(thread: libc::pthread_t) -> Thread {
  // SAFETY: The thread is valid, as asserted by its join handle.
  Thread(unsafe { libc::pthread_mach_thread_np(thread) })
}

fn check(result: kern_return_t) -> Result<()> {
  if result == KERN_SUCCESS {
    Ok(())
  } else {
    Err(Error::Thread(OsError::mach(result)))
  }
}

/// The number of 32-bit words of the thread state.
const STATE_COUNT: mach_msg_type_number_t = (mem::size_of::<State>() / 4) as _;

/// A suspended thread, which is resumed once dropped.
pub(crate) struct Suspended {
  port: thread_act_t,
  /// Whether a reference to the port is owned (and must be released).
  owned: bool,
  state: State,
  modified: bool,
}

impl Suspended {
  /// Suspends the thread of `port`, releasing the port on failure if owned.
  fn new(port: thread_act_t, owned: bool) -> Result<Self> {
    let release = || {
      if owned {
        // SAFETY: A reference to the port is owned.
        unsafe { mach_port_deallocate(mach_task_self(), port) };
      }
    };

    // SAFETY: Suspending a thread has no memory safety implications.
    if let Err(error) = check(unsafe { thread_suspend(port) }) {
      release();
      return Err(error);
    }

    // Retrieving the state waits until the thread has stopped
    // SAFETY: A zeroed thread state is valid.
    let mut state: State = unsafe { mem::zeroed() };
    let mut count = STATE_COUNT;
    // SAFETY: The state buffer holds `count` words.
    let result = check(unsafe {
      thread_get_state(port, FLAVOR, (&raw mut state).cast::<u32>() as thread_state_t, &mut count)
    });
    if let Err(error) = result {
      // SAFETY: The thread was suspended above.
      unsafe { thread_resume(port) };
      release();
      return Err(error);
    }

    Ok(Suspended {
      port,
      owned,
      state,
      modified: false,
    })
  }

  /// Returns the program counter.
  pub fn pc(&self) -> usize {
    #[cfg(target_arch = "aarch64")]
    return self.state.__pc as usize;
    #[cfg(target_arch = "x86_64")]
    return self.state.__rip as usize;
  }

  /// Sets the program counter, which is applied by [`Suspended::apply`].
  pub fn set_pc(&mut self, pc: usize) {
    #[cfg(target_arch = "aarch64")]
    {
      self.state.__pc = pc as u64;
    }
    #[cfg(target_arch = "x86_64")]
    {
      self.state.__rip = pc as u64;
    }
    self.modified = true;
  }

  /// Applies a modified program counter.
  pub(super) fn apply(&mut self) -> Result<()> {
    if !mem::take(&mut self.modified) {
      return Ok(());
    }
    // SAFETY: The state was retrieved from the (suspended) thread.
    check(unsafe {
      thread_set_state(
        self.port,
        FLAVOR,
        (&raw mut self.state).cast::<u32>() as thread_state_t,
        STATE_COUNT,
      )
    })
  }
}

impl Drop for Suspended {
  fn drop(&mut self) {
    // SAFETY: The thread was suspended by `self`, and a reference to the port
    // is released only if owned.
    unsafe {
      thread_resume(self.port);
      if self.owned {
        mach_port_deallocate(mach_task_self(), self.port);
      }
    }
  }
}

/// Suspends all other threads of the task.
///
/// The threads are enumerated repeatedly until no new threads appear. The
/// kernel allocates the enumerated list using virtual memory (not the heap),
/// so this is safe whilst threads are suspended.
pub(super) fn suspend_all() -> Result<Session> {
  // SAFETY: Returns the ports of the current task and thread.
  let (task, this) = unsafe { (mach_task_self(), mach_thread_self()) };
  let mut suspended: Vec<Suspended> = Vec::new();

  let result = loop {
    let mut list: thread_act_array_t = core::ptr::null_mut();
    let mut count: mach_msg_type_number_t = 0;
    // SAFETY: The out-pointers are valid.
    if let Err(error) = check(unsafe { task_threads(task, &mut list, &mut count) }) {
      break Err(error);
    }

    // SAFETY: The kernel returned `count` ports at `list`.
    let ports = unsafe { core::slice::from_raw_parts(list, count as usize) };
    let (mut added, mut complete) = (false, true);

    for &port in ports {
      if port == this || suspended.iter().any(|thread| thread.port == port) {
        // SAFETY: Each enumerated port carries a reference.
        unsafe { mach_port_deallocate(task, port) };
      } else if suspended.len() == suspended.capacity() {
        // SAFETY: See above.
        unsafe { mach_port_deallocate(task, port) };
        complete = false;
      } else if let Ok(thread) = Suspended::new(port, true) {
        // A thread may exit whilst being enumerated; it is ignored
        suspended.push(thread);
        added = true;
      }
    }

    // SAFETY: The list was allocated by the kernel on behalf of the task.
    unsafe {
      mach_vm_deallocate(task, list as u64, u64::from(count) * mem::size_of::<thread_act_t>() as u64)
    };

    if !complete {
      // Resume all threads before allocating, and retry with a larger capacity
      drop(mem::take(&mut suspended));
      suspended = Vec::with_capacity(count as usize * 2 + 16);
    } else if !added {
      break Ok(suspended);
    }
  };

  // SAFETY: Releases the reference returned by `mach_thread_self`.
  unsafe { mach_port_deallocate(task, this) };
  result
}

/// Suspends the given threads, ignoring the current thread.
pub(super) fn suspend(threads: &[Thread]) -> Result<Session> {
  // SAFETY: Returns the ports of the current task and thread.
  let (task, this) = unsafe { (mach_task_self(), mach_thread_self()) };
  let mut suspended = Vec::with_capacity(threads.len());

  let result = threads
    .iter()
    .filter(|thread| thread.0 != this)
    .try_for_each(|thread| Suspended::new(thread.0, false).map(|thread| suspended.push(thread)));

  // SAFETY: Releases the reference returned by `mach_thread_self`.
  unsafe { mach_port_deallocate(task, this) };
  result.map(|()| suspended)
}
