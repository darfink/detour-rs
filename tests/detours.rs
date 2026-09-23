//! Tests of the public API.
use detour::{Error, GenericDetour, RawDetour, Result, static_detour};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod common;

/// A runtime zero, which distinguishes otherwise identical functions (which
/// LLVM would merge, causing concurrent tests to hook the same function).
macro_rules! unique {
  () => {
    std::hint::black_box(0) * line!() as i32
  };
}

type FnAdd = extern "C" fn(i32, i32) -> i32;

#[inline(never)]
extern "C" fn sub_detour(x: i32, y: i32) -> i32 {
  std::hint::black_box(x) - y
}

mod raw {
  use super::*;

  #[test]
  fn basic() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) + y + unique!()
    }

    // SAFETY: The functions share the same signature.
    let hook = unsafe { RawDetour::new(add as *const (), sub_detour as *const ())? };

    assert_eq!(add(10, 5), 15);
    assert!(!hook.is_enabled());

    // SAFETY: No other thread is executing `add`.
    unsafe { hook.enable()? };
    {
      assert!(hook.is_enabled());

      // The `add` function is hooked, but can be called using the trampoline
      // SAFETY: The trampoline shares the target's signature.
      let trampoline: FnAdd = unsafe { std::mem::transmute(hook.trampoline()) };

      assert_eq!(trampoline(10, 5), 15);
      assert_eq!(add(10, 5), 5);
    }
    // SAFETY: No other thread is executing `add`.
    unsafe { hook.disable()? };

    assert!(!hook.is_enabled());
    assert_eq!(add(10, 5), 15);
    Ok(())
  }

  #[test]
  fn reference_arguments() -> Result<()> {
    // Functions with higher-ranked lifetimes are only supported by raw detours
    #[inline(never)]
    fn length(value: &str) -> usize {
      std::hint::black_box(value).len()
    }

    fn zero(_: &str) -> usize {
      0
    }

    // SAFETY: The functions share the same signature.
    let hook = unsafe { RawDetour::new(length as *const (), zero as *const ())? };
    // SAFETY: No other thread is executing `length`.
    unsafe { hook.enable()? };
    assert_eq!(length("detour"), 0);

    // SAFETY: The trampoline shares the target's signature.
    let original: fn(&str) -> usize = unsafe { std::mem::transmute(hook.trampoline()) };
    assert_eq!(original("detour"), 6);
    Ok(())
  }

  #[test]
  fn drop_restores_target() -> Result<()> {
    #[inline(never)]
    extern "C" fn mul(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) * y
    }

    {
      // SAFETY: The functions share the same signature.
      let hook = unsafe { RawDetour::new(mul as *const (), sub_detour as *const ())? };
      // SAFETY: No other thread is executing `mul`.
      unsafe { hook.enable()? };
      assert_eq!(mul(3, 3), 0);
    }

    assert_eq!(mul(3, 3), 9);
    Ok(())
  }

  #[cfg(target_pointer_width = "64")]
  #[test]
  fn distant_detour_uses_relay() -> Result<()> {
    #[inline(never)]
    extern "C" fn ret5() -> i32 {
      std::hint::black_box(5)
    }

    // Beyond ±2 GiB (x86-64), or ±128 MiB (AArch64)
    let distance = if cfg!(target_arch = "aarch64") {
      1 << 28
    } else {
      1 << 32
    };
    let far = common::far_ret10(ret5 as *const () as usize, distance);

    // SAFETY: Both functions share the same signature.
    let hook = unsafe { RawDetour::new(ret5 as *const (), far as *const ())? };
    // SAFETY: No other thread is executing `ret5`.
    unsafe { hook.enable()? };
    assert_eq!(ret5(), 10);

    // SAFETY: The trampoline shares the target's signature.
    let original: extern "C" fn() -> i32 = unsafe { std::mem::transmute(hook.trampoline()) };
    assert_eq!(original(), 5);

    // SAFETY: No other thread is executing `ret5`.
    unsafe { hook.disable()? };
    assert_eq!(ret5(), 5);
    Ok(())
  }

  #[test]
  fn many_detours() -> Result<()> {
    macro_rules! targets {
      ($($name:ident = $value:literal),*) => {{
        $(
          #[inline(never)]
          extern "C" fn $name(x: i32, y: i32) -> i32 {
            std::hint::black_box(x) + y + $value
          }
        )*
        [$($name as FnAdd),*]
      }};
    }

    let targets = targets!(
      a = 1,
      b = 2,
      c = 3,
      d = 4,
      e = 5,
      f = 6,
      g = 7,
      h = 8,
      i = 9,
      j = 10,
      k = 11,
      l = 12,
      m = 13,
      n = 14,
      o = 15,
      p = 16
    );
    let expected: Vec<_> = targets.iter().map(|target| target(1, 1)).collect();

    let hooks = targets
      .iter()
      // SAFETY: The functions share the same signature.
      .map(|target| unsafe { GenericDetour::<FnAdd>::new(*target, sub_detour) })
      .collect::<Result<Vec<_>>>()?;

    for hook in &hooks {
      // SAFETY: No other thread is executing the targets.
      unsafe { hook.enable()? };
    }

    for (index, (target, hook)) in targets.iter().zip(&hooks).enumerate() {
      assert_eq!(target(1, 1), 0);
      assert_eq!(hook.call(1, 1), expected[index]);
    }

    drop(hooks);
    let restored: Vec<_> = targets.iter().map(|target| target(1, 1)).collect();
    assert_eq!(restored, expected);
    Ok(())
  }
}

mod generic {
  use super::*;

  #[test]
  fn toggling_is_idempotent() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) + y + unique!()
    }

    // SAFETY: The functions share the same signature.
    let hook = unsafe { GenericDetour::<FnAdd>::new(add, sub_detour)? };
    // SAFETY: No other thread is executing `add`.
    unsafe {
      hook.enable()?;
      hook.enable()?;
      assert_eq!(add(3, 1), 2);
      hook.disable()?;
      hook.disable()?;
    }
    assert_eq!(add(3, 1), 4);
    Ok(())
  }

  #[test]
  fn memory_is_reused() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) + y + unique!()
    }

    // Trampolines are released upon drop, so this must not exhaust memory
    for _ in 0..10_000 {
      // SAFETY: The functions share the same signature.
      let hook = unsafe { GenericDetour::<FnAdd>::new(add, sub_detour)? };
      // SAFETY: No other thread is executing `add`.
      unsafe { hook.enable()? };
      assert_eq!(add(3, 1), 2);
    }
    assert_eq!(add(3, 1), 4);
    Ok(())
  }

  #[test]
  fn concurrent_creation() {
    if common::is_translated() {
      eprintln!("skipped: concurrent code modification is unreliable under Rosetta 2");
      return;
    }

    macro_rules! targets {
      ($($name:ident = $value:literal),*) => {{
        $(
          #[inline(never)]
          extern "C" fn $name(x: i32, y: i32) -> i32 {
            std::hint::black_box(x) * y + $value
          }
        )*
        [$($name as FnAdd),*]
      }};
    }

    let targets = targets!(
      a = 100,
      b = 200,
      c = 300,
      d = 400,
      e = 500,
      f = 600,
      g = 700,
      h = 800
    );
    let threads: Vec<_> = targets
      .into_iter()
      .map(|target| {
        std::thread::spawn(move || -> Result<()> {
          for _ in 0..50 {
            // SAFETY: Each thread detours a distinct target.
            let hook = unsafe { GenericDetour::<FnAdd>::new(target, sub_detour)? };
            // SAFETY: See above.
            unsafe { hook.enable()? };
            assert_eq!(target(5, 2), 3);
            assert_eq!(hook.call(5, 2), hook.call(0, 0) + 10);
          }
          Ok(())
        })
      })
      .collect();

    for thread in threads {
      thread
        .join()
        .expect("thread panicked")
        .expect("detour failed");
    }
  }

  #[test]
  fn basic() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) + y + unique!()
    }

    // SAFETY: The functions share the same signature.
    let hook = unsafe { GenericDetour::<FnAdd>::new(add, sub_detour)? };

    assert_eq!(add(10, 5), 15);
    assert_eq!(hook.call(10, 5), 15);
    // SAFETY: No other thread is executing `add`.
    unsafe { hook.enable()? };
    {
      assert_eq!(hook.call(10, 5), 15);
      assert_eq!(add(10, 5), 5);
    }
    // SAFETY: No other thread is executing `add`.
    unsafe { hook.disable()? };
    assert_eq!(hook.call(10, 5), 15);
    assert_eq!(add(10, 5), 15);
    Ok(())
  }

  #[test]
  fn unsafe_target_with_safe_detour() -> Result<()> {
    #[inline(never)]
    unsafe extern "C" fn value() -> u64 {
      std::hint::black_box(1)
    }

    extern "C" fn detour() -> u64 {
      2
    }

    // SAFETY: The functions share the same signature.
    let hook = unsafe {
      GenericDetour::<unsafe extern "C" fn() -> u64>::new(value, detour as extern "C" fn() -> u64)?
    };
    // SAFETY: No other thread is executing `value`.
    unsafe {
      hook.enable()?;
      assert_eq!(value(), 2);
      assert_eq!(hook.call(), 1);
    }
    Ok(())
  }

  #[test]
  fn is_send_and_sync() {
    fn assert<T: Send + Sync>() {}
    assert::<GenericDetour<fn()>>();
    assert::<RawDetour>();
    assert::<detour::StaticDetour<fn()>>();
  }
}

mod statik {
  use super::*;

  #[inline(never)]
  unsafe extern "C" fn add(x: i32, y: i32) -> i32 {
    std::hint::black_box(x) + y + unique!()
  }

  #[inline(never)]
  fn counter(value: u64) -> u64 {
    std::hint::black_box(value)
  }

  #[inline(never)]
  extern "system" fn negate(value: i64, scale: i64) -> i64 {
    -std::hint::black_box(value) * scale
  }

  static_detour! {
    #[doc = "Test with attributes"]
    pub static DetourAdd: unsafe extern "C" fn(i32, i32) -> i32;
    static DetourCounter: fn(u64) -> u64;
    pub(crate) static DetourNegate: extern "system" fn(i64, i64,) -> i64;
    static DetourUninitialized: fn();
  }

  #[test]
  fn basic() -> Result<()> {
    // SAFETY: No other thread is executing `add`.
    unsafe {
      DetourAdd.initialize(add, |x, y| x - y)?;

      assert_eq!(add(10, 5), 15);
      assert!(!DetourAdd.is_enabled());

      DetourAdd.enable()?;
      {
        assert!(DetourAdd.is_enabled());
        assert_eq!(DetourAdd.call(10, 5), 15);
        assert_eq!(add(10, 5), 5);
      }
      DetourAdd.disable()?;

      assert!(!DetourAdd.is_enabled());
      assert_eq!(DetourAdd.call(10, 5), 15);
      assert_eq!(add(10, 5), 15);

      assert!(matches!(
        DetourAdd.initialize(add, |x, y| x * y),
        Err(Error::AlreadyInitialized)
      ));
    }
    Ok(())
  }

  #[test]
  fn stateful_closures() -> Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();

    // SAFETY: No other thread is executing `counter`.
    unsafe {
      DetourCounter
        .initialize(counter, move |value| {
          counted.fetch_add(1, Ordering::SeqCst);
          // The original can be called from within the detour
          DetourCounter.call(value) * 2
        })?
        .enable()?;
    }

    assert_eq!(counter(4), 8);
    assert_eq!(counter(5), 10);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    DetourCounter.set_detour(|value| value + 1);
    assert_eq!(counter(4), 5);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // SAFETY: No other thread is executing `counter`.
    unsafe { DetourCounter.disable()? };
    assert_eq!(counter(4), 4);
    Ok(())
  }

  #[test]
  fn system_abi_and_trailing_comma() -> Result<()> {
    // SAFETY: No other thread is executing `negate`.
    unsafe { DetourNegate.initialize(negate, |x, y| x + y)?.enable()? };
    assert_eq!(negate(2, 3), 5);
    assert_eq!(DetourNegate.call(2, 3), -6);
    Ok(())
  }

  #[test]
  fn uninitialized() {
    assert!(!DetourUninitialized.is_enabled());
    // SAFETY: The detour is not initialized.
    let result = unsafe { DetourUninitialized.enable() };
    assert!(matches!(result, Err(Error::NotInitialized)));
    assert!(std::panic::catch_unwind(|| DetourUninitialized.call()).is_err());
  }
}

/// Toggles a detour whilst another thread continuously calls the target.
///
/// A single AArch64 instruction is replaced atomically, so this is safe; on
/// x86 the patch is not guaranteed to be atomic.
#[cfg(target_arch = "aarch64")]
#[test]
fn toggle_whilst_executing() -> Result<()> {
  use std::sync::atomic::AtomicBool;

  #[inline(never)]
  extern "C" fn ret1() -> i32 {
    std::hint::black_box(1)
  }

  extern "C" fn ret2() -> i32 {
    2
  }

  // SAFETY: The functions share the same signature.
  let hook = unsafe { GenericDetour::<extern "C" fn() -> i32>::new(ret1, ret2)? };
  let done = Arc::new(AtomicBool::new(false));

  let calls = Arc::new(AtomicUsize::new(0));

  let worker = std::thread::spawn({
    let (done, calls) = (done.clone(), calls.clone());
    move || {
      let mut results = [0usize; 3];
      while !done.load(Ordering::Relaxed) {
        results[ret1() as usize] += 1;
        calls.fetch_add(1, Ordering::Relaxed);
      }
      results
    }
  });

  // Ensure the worker is executing the target before it is patched
  while calls.load(Ordering::Relaxed) == 0 {
    std::thread::yield_now();
  }

  for _ in 0..200 {
    // SAFETY: Replacing a single instruction is atomic on AArch64.
    unsafe {
      hook.enable()?;
      hook.disable()?;
    }
  }

  done.store(true, Ordering::Relaxed);
  let results = worker.join().expect("worker thread panicked");
  // Every call must return either the original, or the detoured, result
  assert_eq!(results[0], 0);
  assert!(results[1] + results[2] > 0);
  Ok(())
}
