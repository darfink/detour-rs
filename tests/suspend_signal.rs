//! Tests of the Linux suspend signal (in a separate process, since blocking
//! the signal on one thread affects all other suspensions).
#![cfg(any(target_os = "linux", target_os = "android"))]

use detour::{Error, Result, Thread, Threads, Transaction, TypedDetour, set_suspend_signal};
use std::sync::mpsc;

#[inline(never)]
extern "C" fn ret1() -> i32 {
  std::hint::black_box(1)
}

extern "C" fn ret10() -> i32 {
  10
}

#[test]
fn rejects_invalid_signals() {
  assert!(set_suspend_signal(0).is_err());
  assert!(set_suspend_signal(libc::SIGKILL).is_err());
  assert!(set_suspend_signal(libc::SIGSTOP).is_err());
}

/// Commits a transaction enabling a detour of `ret1`, suspending `thread`.
fn commit(thread: &std::thread::JoinHandle<()>) -> Result<()> {
  // SAFETY: The functions share the same signature.
  let hook = unsafe { TypedDetour::<extern "C" fn() -> i32>::new(ret1, ret10)? };
  let threads = [Thread::from(thread)];
  let mut transaction = Transaction::new();
  transaction.enable(&hook);
  // SAFETY: The thread does not execute the target.
  let result = unsafe { transaction.commit(Threads::Only(&threads)) };
  assert!(!hook.is_enabled());
  assert_eq!(ret1(), 1);
  result
}

/// The signal is process-wide state, so the scenarios run sequentially.
#[test]
fn reports_unusable_signals() -> Result<()> {
  // A thread blocking the signal cannot be suspended
  let signal = libc::SIGRTMIN() + 8;
  set_suspend_signal(signal)?;

  let (ready, wait) = mpsc::channel();
  let (stop, stopped) = mpsc::channel::<()>();
  let blocker = std::thread::spawn(move || {
    // SAFETY: Blocks the signal on this thread.
    unsafe {
      let mut set = std::mem::zeroed();
      libc::sigemptyset(&mut set);
      libc::sigaddset(&mut set, signal);
      libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
    ready.send(()).unwrap();
    let _ = stopped.recv();
  });
  wait.recv().unwrap();

  let started = std::time::Instant::now();
  let result = commit(&blocker);
  assert!(matches!(result, Err(Error::Thread(_))), "{result:?}");
  assert!(started.elapsed() < std::time::Duration::from_secs(10));

  // The pending signal is discarded once the thread exits
  stop.send(()).unwrap();
  blocker.join().unwrap();

  // A signal handled by someone else is refused
  extern "C" fn other(_: libc::c_int) {}
  let signal = libc::SIGRTMIN() + 9;
  // SAFETY: Installs a benign handler for an unused signal.
  unsafe { libc::signal(signal, other as *const () as libc::sighandler_t) };
  set_suspend_signal(signal)?;

  let worker = std::thread::spawn(std::thread::park);
  let result = commit(&worker);
  assert!(matches!(result, Err(Error::Thread(_))), "{result:?}");
  worker.thread().unpark();
  worker.join().unwrap();

  // An unused signal works
  set_suspend_signal(libc::SIGRTMIN() + 10)?;
  let worker = std::thread::spawn(std::thread::park);
  // SAFETY: The functions share the same signature.
  let hook = unsafe { TypedDetour::<extern "C" fn() -> i32>::new(ret1, ret10)? };
  let threads = [Thread::from(&worker)];
  let mut transaction = Transaction::new();
  transaction.enable(&hook);
  // SAFETY: The thread does not execute the target.
  unsafe { transaction.commit(Threads::Only(&threads))? };
  assert_eq!(ret1(), 10);
  drop(hook);
  worker.thread().unpark();
  worker.join().unwrap();
  Ok(())
}
