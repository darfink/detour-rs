//! Thread suspension using a real-time signal (Linux & Android).
//!
//! A thread is suspended by sending it a signal. Its handler publishes the
//! interrupted context, and waits (on a futex) until it is released. The
//! program counter of the context may be modified meanwhile; the kernel
//! restores the modified context once the handler returns.
//!
//! Once a thread has been suspended, the suspending thread must not allocate
//! (the suspended thread may hold the allocator's lock), so all storage is
//! allocated in advance, and threads are enumerated using raw system calls.

use super::Thread;
use crate::error::{Error, OsError, Result};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::{c_int, c_void};
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use libc::{c_long, pid_t, timespec, ucontext_t};

/// A `pthread_t`, stored as an integer (it is a pointer on some C libraries,
/// e.g. musl), so `Thread` is `Send + Sync` on all targets.
pub(crate) type RawThread = usize;

/// Creates a thread from a `pthread_t` as an integer (as returned by
/// `JoinHandleExt::as_pthread_t`, which is not a pointer on musl).
#[cfg(feature = "std")]
pub(super) fn from_pthread(thread: usize) -> Thread {
  Thread(thread)
}

/// Returns the `pthread_t` of a thread.
fn pthread(thread: Thread) -> libc::pthread_t {
  thread.0 as libc::pthread_t
}

/// The maximum time to wait for a thread to handle the suspend signal.
const TIMEOUT_NS: i64 = 2_000_000_000;

/// The chosen suspend signal, or zero for the default.
static SIGNAL: AtomicI32 = AtomicI32::new(0);

/// Sets the signal used to suspend threads (by default `SIGRTMIN + 6`).
///
/// The default avoids the lowest real-time signals (commonly used by
/// libraries), and the highest (unavailable under emulators such as QEMU).
///
/// The signal must not be used by anything else in the process, nor be
/// blocked by threads that are suspended. A handler is installed for it the
/// first time threads are suspended, and remains installed thereafter.
pub fn set_suspend_signal(signal: i32) -> Result<()> {
  if signal <= 0 || signal > sigrtmax() || signal == libc::SIGKILL || signal == libc::SIGSTOP
  {
    return Err(Error::Thread(OsError::other("invalid suspend signal")));
  }
  SIGNAL.store(signal, Ordering::SeqCst);
  Ok(())
}

// Declared `unsafe` in older versions of `libc`
#[allow(unused_unsafe)]
fn sigrtmin() -> c_int {
  // SAFETY: Returns a constant of the C library.
  unsafe { libc::SIGRTMIN() }
}

#[allow(unused_unsafe)]
fn sigrtmax() -> c_int {
  // SAFETY: Returns a constant of the C library.
  unsafe { libc::SIGRTMAX() }
}

fn signal() -> c_int {
  match SIGNAL.load(Ordering::SeqCst) {
    0 => sigrtmin() + 6,
    signal => signal,
  }
}

// The states of a slot; transitions are REQUESTED -> STOPPED (by the handler)
// and {REQUESTED, STOPPED} -> RELEASED (by the suspending thread).
const REQUESTED: u32 = 1;
const STOPPED: u32 = 2;
const RELEASED: u32 = 3;

/// The shared state of a thread that is being suspended.
struct Slot {
  /// The thread's identifier, or zero if identified by `pthread`.
  tid: AtomicI32,
  pthread: AtomicUsize,
  state: AtomicU32,
  context: AtomicPtr<ucontext_t>,
}

impl Slot {
  const fn new() -> Self {
    Slot {
      tid: AtomicI32::new(0),
      pthread: AtomicUsize::new(0),
      state: AtomicU32::new(0),
      context: AtomicPtr::new(ptr::null_mut()),
    }
  }

  fn matches(&self, tid: pid_t, pthread: usize) -> bool {
    match self.tid.load(Ordering::Acquire) {
      0 => self.pthread.load(Ordering::Acquire) == pthread,
      id => id == tid,
    }
  }
}

// The slots of the current session (at most one exists at a time, since the
// global patch lock is held), and the number of executing signal handlers.
static SLOTS: AtomicPtr<Slot> = AtomicPtr::new(ptr::null_mut());
static COUNT: AtomicUsize = AtomicUsize::new(0);
static HANDLERS: AtomicUsize = AtomicUsize::new(0);

/// The signal with our handler installed (zero if none).
static INSTALLED: AtomicI32 = AtomicI32::new(0);

#[cfg(target_os = "android")]
use libc::__errno as errno_location;
#[cfg(not(target_os = "android"))]
use libc::__errno_location as errno_location;

fn errno() -> c_int {
  // SAFETY: Reads the calling thread's `errno`.
  unsafe { *errno_location() }
}

fn gettid() -> pid_t {
  // SAFETY: `gettid` has no preconditions.
  unsafe { libc::syscall(libc::SYS_gettid) as pid_t }
}

fn futex_wait(word: &AtomicU32, expected: u32, timeout: Option<&timespec>) {
  const FUTEX_WAIT_PRIVATE: c_int = 128;
  let timeout = timeout.map_or(ptr::null(), |timeout| timeout as *const timespec);
  // SAFETY: The futex word is valid; spurious wake-ups are handled by callers.
  unsafe { libc::syscall(libc::SYS_futex, word.as_ptr(), FUTEX_WAIT_PRIVATE, expected, timeout) };
}

fn futex_wake(word: &AtomicU32) {
  const FUTEX_WAKE_PRIVATE: c_int = 129;
  // SAFETY: The futex word is valid.
  unsafe { libc::syscall(libc::SYS_futex, word.as_ptr(), FUTEX_WAKE_PRIVATE, c_int::MAX) };
}

fn now() -> i64 {
  // SAFETY: A zeroed `timespec` is valid.
  let mut time: timespec = unsafe { core::mem::zeroed() };
  // SAFETY: The out-pointer is valid.
  unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
  time.tv_sec as i64 * 1_000_000_000 + time.tv_nsec as i64
}

/// The suspend signal handler; only async-signal-safe operations are used.
extern "C" fn handler(_signal: c_int, _info: *mut libc::siginfo_t, context: *mut c_void) {
  let saved_errno = errno();
  HANDLERS.fetch_add(1, Ordering::SeqCst);

  let slots = SLOTS.load(Ordering::SeqCst);
  if !slots.is_null() {
    // SAFETY: The slots remain valid whilst `HANDLERS` is non-zero.
    let slots = unsafe { core::slice::from_raw_parts(slots, COUNT.load(Ordering::SeqCst)) };
    // SAFETY: `pthread_self` is async-signal-safe.
    let (tid, pthread) = (gettid(), unsafe { libc::pthread_self() } as usize);

    if let Some(slot) = slots.iter().find(|slot| slot.matches(tid, pthread)) {
      slot.context.store(context.cast(), Ordering::Release);
      let stopped = slot
        .state
        .compare_exchange(REQUESTED, STOPPED, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();

      if stopped {
        futex_wake(&slot.state);
        while slot.state.load(Ordering::Acquire) == STOPPED {
          futex_wait(&slot.state, STOPPED, None);
        }
      }
    }
  }

  HANDLERS.fetch_sub(1, Ordering::SeqCst);
  // SAFETY: Restores the interrupted code's `errno`.
  unsafe { *errno_location() = saved_errno };
}

/// Installs the handler for `signal`, unless it is used by someone else.
fn install(signal: c_int) -> Result<()> {
  if INSTALLED.load(Ordering::SeqCst) == signal {
    return Ok(());
  }

  // SAFETY: A zeroed `sigaction` is valid.
  let mut current: libc::sigaction = unsafe { core::mem::zeroed() };
  // SAFETY: Queries the current action; the out-pointer is valid.
  if unsafe { libc::sigaction(signal, ptr::null(), &mut current) } != 0 {
    return Err(Error::Thread(OsError::from_errno(errno())));
  }
  let ours = handler as extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) as usize;
  if ![libc::SIG_DFL, libc::SIG_IGN, ours].contains(&current.sa_sigaction) {
    return Err(Error::Thread(OsError::other("the suspend signal is already in use")));
  }

  // SAFETY: See above.
  let mut action: libc::sigaction = unsafe { core::mem::zeroed() };
  action.sa_sigaction = ours;
  action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
  // SAFETY: Blocks all other signals whilst the handler executes.
  unsafe { libc::sigfillset(&mut action.sa_mask) };
  // SAFETY: The action is valid, and the handler is async-signal-safe.
  if unsafe { libc::sigaction(signal, &action, ptr::null_mut()) } != 0 {
    return Err(Error::Thread(OsError::from_errno(errno())));
  }
  INSTALLED.store(signal, Ordering::SeqCst);
  Ok(())
}

/// A suspended thread, whose context is modified in place.
pub(crate) struct Suspended {
  context: *mut ucontext_t,
}

impl Suspended {
  /// Returns the program counter.
  pub fn pc(&self) -> usize {
    // SAFETY: The context is valid whilst the thread is suspended.
    let context = unsafe { &(*self.context).uc_mcontext };
    #[cfg(target_arch = "x86_64")]
    return context.gregs[libc::REG_RIP as usize] as usize;
    #[cfg(target_arch = "x86")]
    return context.gregs[libc::REG_EIP as usize] as u32 as usize;
    #[cfg(target_arch = "aarch64")]
    return context.pc as usize;
  }

  /// Sets the program counter, restored once the thread is resumed.
  pub fn set_pc(&mut self, pc: usize) {
    // SAFETY: The context is valid whilst the thread is suspended, and only
    // accessed by the handler once released.
    let context = unsafe { &mut (*self.context).uc_mcontext };
    #[cfg(target_arch = "x86_64")]
    {
      context.gregs[libc::REG_RIP as usize] = pc as _;
    }
    #[cfg(target_arch = "x86")]
    {
      context.gregs[libc::REG_EIP as usize] = pc as u32 as _;
    }
    #[cfg(target_arch = "aarch64")]
    {
      context.pc = pc as _;
    }
  }

  /// The context is modified in place, so this is a no-op.
  pub(super) fn apply(&mut self) -> Result<()> {
    Ok(())
  }
}

/// The suspended threads, resumed once dropped.
#[derive(Default)]
pub(crate) struct Session {
  slots: Box<[Slot]>,
  /// The number of slots in use (published to the handler).
  count: usize,
  threads: Vec<Suspended>,
}

impl Session {
  /// Allocates storage for `capacity` threads, and publishes it.
  fn new(capacity: usize) -> Self {
    let session = Session {
      slots: (0..capacity).map(|_| Slot::new()).collect(),
      count: 0,
      threads: Vec::with_capacity(capacity),
    };
    COUNT.store(0, Ordering::SeqCst);
    SLOTS.store(session.slots.as_ptr().cast_mut(), Ordering::SeqCst);
    session
  }

  /// Requests the suspension of a thread, returning false if it has exited.
  fn request(&mut self, tid: pid_t, pthread: Option<libc::pthread_t>) -> Result<bool> {
    let slot = &self.slots[self.count];
    slot.tid.store(tid, Ordering::Release);
    slot.pthread.store(pthread.map_or(0, |thread| thread as usize), Ordering::Release);
    slot.state.store(REQUESTED, Ordering::Release);
    self.count += 1;
    COUNT.store(self.count, Ordering::SeqCst);

    let signal = signal();
    let result = match pthread {
      // SAFETY: The thread is valid, as guaranteed by the caller.
      Some(thread) => unsafe { libc::pthread_kill(thread, signal) },
      None => {
        // SAFETY: Signals a thread of the current process.
        let result = unsafe {
          libc::syscall(libc::SYS_tgkill, libc::getpid() as c_long, tid as c_long, signal as c_long)
        };
        if result == 0 { 0 } else { errno() }
      },
    };

    match result {
      0 => Ok(true),
      libc::ESRCH if pthread.is_none() => {
        slot.state.store(RELEASED, Ordering::Release);
        Ok(false)
      },
      code => {
        slot.state.store(RELEASED, Ordering::Release);
        Err(Error::Thread(OsError::from_errno(code)))
      },
    }
  }

  /// Waits until the threads of `slots` have stopped.
  ///
  /// Threads identified by `tid` (i.e. enumerated) that exit, or block the
  /// signal, meanwhile are skipped.
  fn wait(&mut self, slots: core::ops::Range<usize>) -> Result<()> {
    const POLL_NS: i64 = 1_000_000;
    let deadline = now() + TIMEOUT_NS;
    let signal = signal();

    for slot in &self.slots[slots] {
      loop {
        match slot.state.load(Ordering::Acquire) {
          STOPPED => {
            let context = slot.context.load(Ordering::Acquire);
            self.threads.push(Suspended { context });
            break;
          },
          REQUESTED => {
            let tid = slot.tid.load(Ordering::Relaxed);
            let remaining = deadline - now();
            if tid != 0 && status(tid, signal, true) == Status::Unreachable {
              // The thread cannot execute any code (other than exiting)
              // until it unblocks the signal, at which point it is released.
              let _ = slot.state.compare_exchange(
                REQUESTED,
                RELEASED,
                Ordering::AcqRel,
                Ordering::Acquire,
              );
              continue;
            }
            if remaining <= 0 {
              return Err(Error::Thread(OsError::other(
                "a thread did not handle the suspend signal (it may be blocked)",
              )));
            }
            let wait = remaining.min(POLL_NS);
            let timeout = timespec {
              tv_sec: (wait / 1_000_000_000) as _,
              tv_nsec: (wait % 1_000_000_000) as _,
            };
            futex_wait(&slot.state, REQUESTED, Some(&timeout));
          },
          _ => break,
        }
      }
    }
    Ok(())
  }

  fn contains(&self, tid: pid_t) -> bool {
    self.slots[..self.count].iter().any(|slot| slot.tid.load(Ordering::Relaxed) == tid)
  }

  fn is_full(&self) -> bool {
    self.count == self.slots.len()
  }
}

impl Deref for Session {
  type Target = [Suspended];

  fn deref(&self) -> &Self::Target {
    &self.threads
  }
}

impl DerefMut for Session {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.threads
  }
}

impl Drop for Session {
  fn drop(&mut self) {
    if self.slots.is_empty() {
      return;
    }

    // Released slots are ignored by late signals (e.g. of a thread that timed out)
    for slot in &self.slots[..self.count] {
      if slot.state.swap(RELEASED, Ordering::AcqRel) == STOPPED {
        futex_wake(&slot.state);
      }
    }

    // The slots are released once no handler may access them
    SLOTS.store(ptr::null_mut(), Ordering::SeqCst);
    while HANDLERS.load(Ordering::SeqCst) != 0 {
      // SAFETY: `sched_yield` has no preconditions.
      unsafe { libc::sched_yield() };
    }
    self.threads.clear();
  }
}

/// The state of a thread with regard to the suspend signal.
#[derive(PartialEq, Eq)]
enum Status {
  /// The thread may handle the signal.
  Receptive,
  /// The thread blocks the signal (e.g. whilst exiting), or has exited.
  Unreachable,
}

/// Returns whether thread `tid` may handle `signal`, without allocating.
///
/// If `sent`, the signal has been sent to the thread, so it is unreachable
/// only if the signal is still pending. A thread executing the handler blocks
/// all signals, but has no pending signal.
fn status(tid: pid_t, signal: c_int, sent: bool) -> Status {
  // "/proc/self/task/<tid>/status"
  let mut path = [0u8; 48];
  let mut len = 0;
  let mut push = |bytes: &[u8]| {
    path[len..len + bytes.len()].copy_from_slice(bytes);
    len += bytes.len();
  };
  push(b"/proc/self/task/");
  let mut digits = [0u8; 10];
  let mut count = 0;
  let mut value = tid as u32;
  loop {
    digits[count] = b'0' + (value % 10) as u8;
    count += 1;
    value /= 10;
    if value == 0 {
      break;
    }
  }
  digits[..count].reverse();
  push(&digits[..count]);
  push(b"/status\0");

  // SAFETY: The path is a valid C string.
  let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
  if fd < 0 {
    return Status::Unreachable;
  }
  let mut buffer = [0u8; 4096];
  // SAFETY: The buffer is valid for writes of its size.
  let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
  // SAFETY: The descriptor is owned.
  unsafe { libc::close(fd) };
  if read <= 0 {
    return Status::Unreachable;
  }

  // "<key>\t<64-bit hexadecimal mask>"; bit `n - 1` represents signal `n`
  let status = &buffer[..read as usize];
  let contains = |key: &[u8]| {
    status
      .split(|&byte| byte == b'\n')
      .find_map(|line| line.strip_prefix(key))
      .and_then(|mask| {
        mask.iter().try_fold(0u64, |mask, &digit| {
          char::from(digit).to_digit(16).map(|digit| mask << 4 | u64::from(digit))
        })
      })
      .map(|mask| mask & (1 << (signal - 1)) != 0)
  };

  // An unexpected format is assumed to be receptive
  let blocked = contains(b"SigBlk:\t").unwrap_or(false);
  let pending = !sent || contains(b"SigPnd:\t").unwrap_or(false);
  if blocked && pending { Status::Unreachable } else { Status::Receptive }
}

/// Calls `visit` with each thread identifier of the process, without
/// allocating.
fn for_each_tid(mut visit: impl FnMut(pid_t) -> Result<()>) -> Result<()> {
  // SAFETY: Opens a directory; the path is a valid C string.
  let fd = unsafe {
    libc::open(c"/proc/self/task".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC)
  };
  if fd < 0 {
    return Err(Error::Thread(OsError::from_errno(errno())));
  }

  let mut buffer = [0u64; 512];
  let result = (|| loop {
    // SAFETY: The buffer is valid for writes of its size.
    let len = unsafe {
      libc::syscall(libc::SYS_getdents64, fd, buffer.as_mut_ptr(), size_of_val(&buffer))
    };
    if len < 0 {
      return Err(Error::Thread(OsError::from_errno(errno())));
    }
    if len == 0 {
      return Ok(());
    }

    // SAFETY: The kernel wrote `len` bytes of `linux_dirent64` records.
    let bytes = unsafe { core::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), len as usize) };
    let mut offset = 0;
    while offset < bytes.len() {
      // struct linux_dirent64 { u64 ino; i64 off; u16 reclen; u8 type; char name[]; }
      let record = u16::from_ne_bytes([bytes[offset + 16], bytes[offset + 17]]) as usize;
      let name = bytes[offset + 19..offset + record].split(|&byte| byte == 0).next();
      let tid = name.and_then(|name| {
        name.iter().try_fold(0 as pid_t, |tid, &digit| {
          digit.is_ascii_digit().then(|| tid * 10 + pid_t::from(digit - b'0'))
        })
      });
      if let Some(tid) = tid.filter(|&tid| tid > 0) {
        visit(tid)?;
      }
      offset += record;
    }
  })();

  // SAFETY: The descriptor is owned.
  unsafe { libc::close(fd) };
  result
}

/// Suspends all other threads of the process.
///
/// The threads are enumerated repeatedly until no new threads appear.
/// Threads blocking the signal (e.g. helper threads blocking all signals, or
/// exiting threads) cannot be suspended, and are skipped.
pub(super) fn suspend_all() -> Result<Session> {
  let signal = signal();
  install(signal)?;
  let current = gettid();
  let mut capacity = 64;

  loop {
    let mut session = Session::new(capacity);
    let mut complete = true;
    let mut total;

    loop {
      let first = session.count;
      total = 0;
      for_each_tid(|tid| {
        if tid == current {
          return Ok(());
        }
        total += 1;
        if !session.contains(tid) && status(tid, signal, false) == Status::Receptive {
          if session.is_full() {
            complete = false;
          } else {
            session.request(tid, None)?;
          }
        }
        Ok(())
      })?;

      let added = session.count - first;
      session.wait(first..session.count)?;
      if !complete || added == 0 {
        break;
      }
    }

    if complete {
      return Ok(session);
    }
    // Resume all threads before allocating, and retry with a larger capacity
    drop(session);
    capacity = total * 2 + 16;
  }
}

/// Suspends the given threads, ignoring the current thread.
pub(super) fn suspend(threads: &[Thread]) -> Result<Session> {
  if threads.is_empty() {
    return Ok(Session::default());
  }
  install(signal())?;

  // SAFETY: `pthread_self` has no preconditions.
  let current = unsafe { libc::pthread_self() };
  let mut session = Session::new(threads.len());
  for thread in threads {
    // SAFETY: Comparing thread identifiers has no preconditions.
    if unsafe { libc::pthread_equal(pthread(*thread), current) } == 0 {
      session.request(0, Some(pthread(*thread)))?;
    }
  }
  session.wait(0..session.count)?;
  Ok(session)
}
