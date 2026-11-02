//! Transactions, which enable and disable several detours at once.

use crate::error::{Error, Result};
use crate::hook::Hook;
use crate::memory;
use crate::thread::{Suspended, Threads};
use alloc::vec::Vec;

/// A detour, which can be part of a [`Transaction`].
///
/// This is implemented by all detour types, and cannot be implemented
/// outside of this crate. Different detours can be grouped as `&dyn Detour`:
///
/// ```rust
/// use detour::{Detour, TypedDetour, static_detour};
///
/// static_detour! {
///   static Unused: fn(i32) -> i32;
/// }
///
/// #[inline(never)]
/// fn add5(val: i32) -> i32 {
///   val + 5
/// }
///
/// fn add10(val: i32) -> i32 {
///   val + 10
/// }
///
/// # fn main() -> detour::Result<()> {
/// let hook = unsafe { TypedDetour::<fn(i32) -> i32>::new(add5, add10)? };
/// unsafe { hook.enable()? };
///
/// let detours: [&dyn Detour; 2] = [&hook, &Unused];
/// assert_eq!(detours.map(|detour| detour.is_enabled()), [true, false]);
/// # Ok(())
/// # }
/// ```
pub trait Detour: private::Sealed {
  /// Returns whether the detour is enabled or not.
  ///
  /// An uninitialized static detour is not enabled.
  fn is_enabled(&self) -> bool {
    self.hook().is_some_and(Hook::is_enabled)
  }
}

pub(crate) mod private {
  use crate::hook::Hook;

  pub trait Sealed {
    /// Returns the underlying hook, if initialized.
    fn hook(&self) -> Option<&Hook>;
  }
}

/// A set of detours to enable and disable at once.
///
/// When committed, the detours are toggled in the order they were added,
/// optionally whilst other threads are suspended (see [`Threads`]). A
/// suspended thread executing instructions that are patched is moved to
/// equivalent code (known as EIP relocation). If any change fails, all
/// preceding changes are reverted, so a transaction is applied either
/// completely, or not at all.
///
/// Only one transaction is committed at a time (as are all other code
/// modifications performed by this crate).
///
/// # Example
///
/// ```rust
/// # use detour::Result;
/// use detour::{Threads, Transaction, TypedDetour};
///
/// #[inline(never)]
/// fn add5(val: i32) -> i32 {
///   val + 5
/// }
///
/// #[inline(never)]
/// fn add10(val: i32) -> i32 {
///   val + 10
/// }
///
/// fn sub5(val: i32) -> i32 {
///   val - 5
/// }
///
/// # fn main() -> Result<()> {
/// // SAFETY: The functions share the same signature.
/// let (hook1, hook2) = unsafe {
///   (
///     TypedDetour::<fn(i32) -> i32>::new(add5, sub5)?,
///     TypedDetour::<fn(i32) -> i32>::new(add10, sub5)?,
///   )
/// };
///
/// let mut transaction = Transaction::new();
/// transaction.enable(&hook1).enable(&hook2);
/// // SAFETY: All other threads are suspended, so none can observe the patch
/// // being written.
/// unsafe { transaction.commit(Threads::All)? };
///
/// assert_eq!((add5(10), add10(10)), (5, 5));
/// # Ok(())
/// # }
/// ```
#[derive(Default)]
#[must_use = "a transaction has no effect until committed"]
pub struct Transaction<'a> {
  operations: Vec<(Option<&'a Hook>, bool)>,
}

impl<'a> Transaction<'a> {
  /// Creates an empty transaction.
  pub fn new() -> Self {
    Self::default()
  }

  /// Enables `detour` once committed.
  pub fn enable(&mut self, detour: &'a (impl Detour + ?Sized)) -> &mut Self {
    self.operations.push((detour.hook(), true));
    self
  }

  /// Disables `detour` once committed.
  pub fn disable(&mut self, detour: &'a (impl Detour + ?Sized)) -> &mut Self {
    self.operations.push((detour.hook(), false));
    self
  }

  /// Commits the transaction, whilst `threads` are suspended.
  ///
  /// Suspended threads executing any patched instructions are moved to
  /// equivalent code: when enabling, to the same instruction in the
  /// trampoline; when disabling, from the midst of the patch back to the
  /// start of the target. If a thread cannot be moved (e.g. it is stopped at
  /// an instruction that was rewritten when relocated),
  /// [`Error::ThreadNotRelocatable`] is returned, and the transaction is reverted.
  ///
  /// Suspending threads is supported on Windows, Apple platforms, Linux and
  /// Android; on other platforms, [`Error::ThreadsUnsupported`] is returned
  /// unless [`Threads::None`] is used.
  ///
  /// # Safety
  ///
  /// - Threads that are not suspended must not execute the patched
  ///   instructions (i.e. the prologs of the targets) whilst the transaction
  ///   is committed.
  /// - Each thread of [`Threads::Only`] must be a thread of the current
  ///   process, which remains valid (i.e. is not joined, or exits whilst
  ///   detached) until this returns.
  ///
  /// # Limitations
  ///
  /// Only the program counters of suspended threads are relocated. A return
  /// address referring to patched instructions (e.g. of a thread that is
  /// executing a function called from within a target's prolog) is not
  /// adjusted; such a thread returns into the midst of the patch.
  pub unsafe fn commit(self, threads: Threads<'_>) -> Result<()> {
    let operations = self
      .operations
      .into_iter()
      .map(|(hook, enable)| hook.map(|hook| (hook, enable)).ok_or(Error::NotInitialized))
      .collect::<Result<Vec<_>>>()?;

    // SAFETY: Forwarded from the caller.
    unsafe { commit(&operations, threads) }
  }
}

impl core::fmt::Debug for Transaction<'_> {
  fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    f.debug_struct("Transaction")
      .field("operations", &self.operations.len())
      .finish()
  }
}

/// Enables or disables hooks, whilst `threads` are suspended.
///
/// # Safety
///
/// See [`Transaction::commit`].
pub(crate) unsafe fn commit(operations: &[(&Hook, bool)], threads: Threads<'_>) -> Result<()> {
  let mut registry = memory::patch_lock();

  // Everything requiring allocations is prepared before threads are suspended
  let protections = operations
    .iter()
    .map(|(hook, _)| hook.protections())
    .collect::<Result<Vec<_>>>()?;
  for (hook, _) in operations {
    registry.reserve(hook, operations.len());
  }
  let mut applied = Vec::with_capacity(operations.len());

  let mut suspended = Suspended::new(threads)?;
  let result = (|| {
    for (index, &(hook, enable)) in operations.iter().enumerate() {
      if hook.is_enabled() == enable {
        continue;
      }

      // Verify that all threads can be moved, before modifying any code
      let change = hook.change(&mut registry, enable);
      for thread in suspended.threads() {
        hook.relocate(thread.pc(), change)?;
      }

      // SAFETY: Unsuspended threads do not execute the patch (as guaranteed
      // by the caller), and suspended threads are moved below.
      let (undo, result) = unsafe { hook.apply(&mut registry, enable, &protections[index]) };
      // An operation applied despite an error is reverted as well
      if let Some(undo) = undo {
        applied.push((index, undo));
      }
      result?;

      for thread in suspended.threads() {
        if let Some(pc) = hook.relocate(thread.pc(), change)? {
          thread.set_pc(pc);
        }
      }
    }
    suspended.apply()
  })();

  if result.is_err() {
    for &(index, undo) in applied.iter().rev() {
      // SAFETY: Reverts the operations applied above, in reverse order. The
      // code cannot have been modified since, so this does not fail unless
      // the system call does.
      unsafe { operations[index].0.revert(&mut registry, undo, &protections[index]) };
    }
  }

  // The threads are resumed before the vectors are released
  drop(suspended);
  result
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::RawDetour;
  use crate::memory::FAIL_AFTER_WRITE;
  use core::arch::naked_asm;

  type FnRet = unsafe extern "C" fn() -> i32;

  extern "C" fn ret1() -> i32 {
    1
  }

  extern "C" fn ret2() -> i32 {
    2
  }

  extern "C" fn ret3() -> i32 {
    3
  }

  /// Detours `target` to `detour`.
  fn detour(target: FnRet, detour: extern "C" fn() -> i32) -> Result<RawDetour> {
    // SAFETY: Both functions share the same signature.
    unsafe { RawDetour::new(target as *const (), detour as *const ()) }
  }

  /// Calls the original function of `detour`.
  fn call(detour: &RawDetour) -> i32 {
    // SAFETY: The trampoline shares the signature of `FnRet`.
    unsafe { core::mem::transmute::<*const (), FnRet>(detour.trampoline())() }
  }

  /// Commits `transaction`, failing once `writes` code patches have succeeded.
  fn commit_failing(transaction: Transaction<'_>, writes: usize) -> Result<()> {
    FAIL_AFTER_WRITE.set(Some(writes));
    // SAFETY: No other thread is executing the targets.
    let result = unsafe { transaction.commit(Threads::None) };
    FAIL_AFTER_WRITE.set(None);
    result
  }

  #[test]
  fn reverts_all_changes_on_failure() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn ret4() -> i32 {
      #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
      naked_asm!("mov eax, 4", "ret");
      #[cfg(target_arch = "aarch64")]
      naked_asm!("mov w0, #4", "ret");
    }

    #[unsafe(naked)]
    unsafe extern "C" fn ret5() -> i32 {
      #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
      naked_asm!("mov eax, 5", "ret");
      #[cfg(target_arch = "aarch64")]
      naked_asm!("mov w0, #5", "ret");
    }

    let (hook1, hook2) = (detour(ret4, ret1)?, detour(ret5, ret2)?);
    let mut transaction = Transaction::new();
    transaction.enable(&hook1).enable(&hook2);

    // The second patch fails, after which both are reverted
    assert!(commit_failing(transaction, 1).is_err());
    assert!(!hook1.is_enabled() && !hook2.is_enabled());
    // SAFETY: The targets have no preconditions.
    assert_eq!(unsafe { (ret4(), ret5()) }, (4, 5));
    Ok(())
  }

  #[test]
  fn reverts_chains_on_failure() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn ret6() -> i32 {
      #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
      naked_asm!("mov eax, 6", "ret");
      #[cfg(target_arch = "aarch64")]
      naked_asm!("mov w0, #6", "ret");
    }

    let hooks = [detour(ret6, ret1)?, detour(ret6, ret2)?, detour(ret6, ret3)?];
    let mut transaction = Transaction::new();
    transaction.enable(&hooks[0]).enable(&hooks[1]).enable(&hooks[2]);
    // SAFETY: No other thread is executing `ret6`.
    unsafe { transaction.commit(Threads::None)? };

    // Each detour's original function is the one enabled before it
    // SAFETY: `ret6` has no preconditions.
    let state = || (unsafe { ret6() }, hooks.each_ref().map(call));
    assert_eq!(state(), (3, [6, 1, 2]));

    // Unlinking the middle detour succeeds, then redirecting the target fails
    let mut transaction = Transaction::new();
    transaction.disable(&hooks[1]).disable(&hooks[2]);
    assert!(commit_failing(transaction, 0).is_err());
    assert!(hooks.iter().all(|hook| hook.is_enabled()));
    assert_eq!(state(), (3, [6, 1, 2]));

    // Disabled detours call the original function
    let mut transaction = Transaction::new();
    transaction.disable(&hooks[1]);
    // SAFETY: See above.
    unsafe { transaction.commit(Threads::None)? };
    assert_eq!(state(), (3, [6, 6, 1]));
    Ok(())
  }
}
