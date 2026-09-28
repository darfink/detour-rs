//! Tests of transactions, and thread suspension.
use detour::{Error, Result, Threads, Transaction, TypedDetour, static_detour};

type FnRet = extern "C" fn() -> i32;

extern "C" fn ret10() -> i32 {
  10
}

#[test]
fn enables_and_disables_several_detours() -> Result<()> {
  #[inline(never)]
  extern "C" fn ret1() -> i32 {
    std::hint::black_box(1)
  }

  #[inline(never)]
  extern "C" fn ret2() -> i32 {
    std::hint::black_box(2)
  }

  // SAFETY: The functions share the same signature.
  let (hook1, hook2) = unsafe {
    (
      TypedDetour::<FnRet>::new(ret1, ret10)?,
      TypedDetour::<FnRet>::new(ret2, ret10)?,
    )
  };

  let mut transaction = Transaction::new();
  transaction.enable(&hook1).enable(&hook2);
  // SAFETY: No other thread is executing the targets.
  unsafe { transaction.commit(Threads::None)? };
  assert!(hook1.is_enabled() && hook2.is_enabled());
  assert_eq!((ret1(), ret2()), (10, 10));

  let mut transaction = Transaction::new();
  transaction.disable(&hook1).disable(&hook2);
  // SAFETY: See above.
  unsafe { transaction.commit(Threads::None)? };
  assert_eq!((ret1(), ret2()), (1, 2));
  Ok(())
}

#[test]
fn applies_operations_in_order() -> Result<()> {
  #[inline(never)]
  extern "C" fn ret3() -> i32 {
    std::hint::black_box(3)
  }

  // SAFETY: The functions share the same signature.
  let hook = unsafe { TypedDetour::<FnRet>::new(ret3, ret10)? };

  let mut transaction = Transaction::new();
  transaction.enable(&hook).disable(&hook).enable(&hook);
  // SAFETY: No other thread is executing the target.
  unsafe { transaction.commit(Threads::None)? };
  assert_eq!(ret3(), 10);
  Ok(())
}

#[test]
fn reverts_all_changes_on_failure() -> Result<()> {
  #[inline(never)]
  extern "C" fn ret4() -> i32 {
    std::hint::black_box(4)
  }

  #[inline(never)]
  extern "C" fn ret5() -> i32 {
    std::hint::black_box(5)
  }

  // SAFETY: The functions share the same signature.
  let (hook1, hook2, hook3) = unsafe {
    (
      TypedDetour::<FnRet>::new(ret4, ret10)?,
      TypedDetour::<FnRet>::new(ret5, ret10)?,
      TypedDetour::<FnRet>::new(ret5, ret4)?,
    )
  };

  // Both detours of `ret5` expect its original code, so the second fails
  let mut transaction = Transaction::new();
  transaction.enable(&hook1).enable(&hook2).enable(&hook3);
  // SAFETY: No other thread is executing the targets.
  let result = unsafe { transaction.commit(Threads::None) };
  assert_eq!(result, Err(Error::TargetModified));

  assert!(!hook1.is_enabled() && !hook2.is_enabled() && !hook3.is_enabled());
  assert_eq!((ret4(), ret5()), (4, 5));
  Ok(())
}

#[test]
fn requires_initialized_detours() -> Result<()> {
  #[inline(never)]
  extern "C" fn ret6() -> i32 {
    std::hint::black_box(6)
  }

  static_detour! {
    static Uninitialized: extern "C" fn() -> i32;
  }

  // SAFETY: The functions share the same signature.
  let hook = unsafe { TypedDetour::<FnRet>::new(ret6, ret10)? };

  let mut transaction = Transaction::new();
  transaction.enable(&hook).enable(&Uninitialized);
  // SAFETY: No other thread is executing the target.
  let result = unsafe { transaction.commit(Threads::None) };
  assert_eq!(result, Err(Error::NotInitialized));
  assert_eq!(ret6(), 6);
  Ok(())
}

#[cfg(not(any(
  windows,
  target_vendor = "apple",
  target_os = "linux",
  target_os = "android"
)))]
#[test]
fn threads_are_unsupported() -> Result<()> {
  #[inline(never)]
  extern "C" fn ret7() -> i32 {
    std::hint::black_box(7)
  }

  // SAFETY: The functions share the same signature.
  let hook = unsafe { TypedDetour::<FnRet>::new(ret7, ret10)? };
  let mut transaction = Transaction::new();
  transaction.enable(&hook);
  // SAFETY: No thread is suspended.
  let result = unsafe { transaction.commit(Threads::All) };
  assert_eq!(result, Err(Error::ThreadsUnsupported));
  assert!(!hook.is_enabled());
  Ok(())
}

/// Tests of thread suspension, and instruction pointer relocation.
///
/// On AArch64, a single instruction is patched (unless no memory is available
/// nearby), so relocation is tested using the absolute patch in unit tests.
#[cfg(all(
  any(target_arch = "x86", target_arch = "x86_64"),
  any(
    windows,
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android"
  )
))]
mod suspension {
  use super::*;
  use detour::Thread;
  use std::arch::naked_asm;
  use std::sync::Arc;
  use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
  use std::thread::JoinHandle;

  const COUNT: usize = 10_000;

  /// Spins `count` times within its prolog (i.e. the patched instructions),
  /// and returns `5`. The count is passed in `rcx`/`ecx`, used by `loop`.
  #[cfg(target_arch = "x86_64")]
  #[unsafe(naked)]
  extern "win64" fn spin(_count: usize) -> i32 {
    naked_asm!("2:", "pause", "loop 2b", "mov eax, 5", "ret")
  }

  /// See above.
  #[cfg(target_arch = "x86")]
  #[unsafe(naked)]
  extern "fastcall" fn spin(_count: usize) -> i32 {
    naked_asm!("2:", "pause", "loop 2b", "mov eax, 5", "ret")
  }

  #[cfg(target_arch = "x86_64")]
  type FnSpin = extern "win64" fn(usize) -> i32;
  #[cfg(target_arch = "x86")]
  type FnSpin = extern "fastcall" fn(usize) -> i32;

  /// Returns `10`, or `99` if entered from the midst of `spin` (i.e. a thread
  /// was not relocated, and branched to the patch).
  #[cfg(target_arch = "x86_64")]
  extern "win64" fn spin_detour(count: usize) -> i32 {
    if count == COUNT { 10 } else { 99 }
  }
  /// See above.
  #[cfg(target_arch = "x86")]
  extern "fastcall" fn spin_detour(count: usize) -> i32 {
    if count == COUNT { 10 } else { 99 }
  }

  struct Worker {
    handle: JoinHandle<()>,
    done: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
  }

  impl Worker {
    /// Spawns a thread continuously calling `spin`.
    fn spawn() -> Self {
      let done = Arc::new(AtomicBool::new(false));
      let calls = Arc::new(AtomicUsize::new(0));
      let handle = std::thread::spawn({
        let (done, calls) = (done.clone(), calls.clone());
        move || {
          while !done.load(Ordering::Relaxed) {
            let result = spin(std::hint::black_box(COUNT));
            assert!(result == 5 || result == 10, "unexpected result: {result}");
            calls.fetch_add(1, Ordering::Relaxed);
          }
        }
      });

      // Ensure the worker is executing the target before it is patched
      while calls.load(Ordering::Relaxed) == 0 {
        std::thread::yield_now();
      }
      Worker {
        handle,
        done,
        calls,
      }
    }

    /// Waits until the worker has completed another call.
    fn wait(&self) {
      let calls = self.calls.load(Ordering::Relaxed);
      while self.calls.load(Ordering::Relaxed) == calls {
        assert!(!self.handle.is_finished(), "worker thread panicked");
        std::thread::yield_now();
      }
    }

    fn join(self) {
      self.done.store(true, Ordering::Relaxed);
      self.handle.join().expect("worker thread panicked");
    }
  }

  /// Toggles the detour of `spin` whilst a worker executes its prolog.
  fn toggle_whilst_spinning(only_worker: bool) -> Result<()> {
    // SAFETY: The functions share the same signature.
    let hook = unsafe { TypedDetour::<FnSpin>::new(spin, spin_detour)? };
    let worker = Worker::spawn();
    let only = [Thread::from(&worker.handle)];
    let threads = if only_worker {
      Threads::Only(&only)
    } else {
      Threads::All
    };

    for _ in 0..200 {
      for enable in [true, false] {
        let mut transaction = Transaction::new();
        if enable {
          transaction.enable(&hook);
        } else {
          transaction.disable(&hook);
        }

        // SAFETY: The worker is suspended, and its instruction pointer is
        // relocated if it is executing the patched instructions.
        unsafe { transaction.commit(threads)? };
      }
      worker.wait();
    }

    worker.join();
    assert_eq!(spin(COUNT), 5);
    Ok(())
  }

  /// Both modes share `spin`, so they run sequentially.
  #[test]
  fn relocates_suspended_threads() -> Result<()> {
    toggle_whilst_spinning(false)?;
    toggle_whilst_spinning(true)
  }
}

#[cfg(any(
  windows,
  target_vendor = "apple",
  target_os = "linux",
  target_os = "android"
))]
#[test]
fn ignores_the_current_thread() -> Result<()> {
  #[inline(never)]
  extern "C" fn ret8() -> i32 {
    std::hint::black_box(8)
  }

  // SAFETY: The functions share the same signature.
  let hook = unsafe { TypedDetour::<FnRet>::new(ret8, ret10)? };
  let mut transaction = Transaction::new();
  transaction.enable(&hook);
  // SAFETY: All other threads are suspended.
  unsafe { transaction.commit(Threads::All)? };
  assert_eq!(ret8(), 10);
  Ok(())
}
