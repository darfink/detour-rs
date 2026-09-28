//! Transactions, which enable and disable several detours at once.

use crate::error::{Error, Result};
use crate::hook::Hook;
use crate::memory;
use crate::thread::{Suspended, Threads};
use alloc::vec::Vec;

/// A detour, which can be part of a [`Transaction`].
///
/// This is implemented by all detour types, and cannot be implemented
/// outside of this crate.
pub trait Detour: private::Sealed {}

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
  let _lock = memory::patch_lock();

  // Everything requiring allocations is prepared before threads are suspended
  let protections = operations
    .iter()
    .map(|(hook, _)| hook.protections())
    .collect::<Result<Vec<_>>>()?;
  let mut applied = Vec::with_capacity(operations.len());

  let mut suspended = Suspended::new(threads)?;
  let result = (|| {
    for (index, &(hook, enable)) in operations.iter().enumerate() {
      if hook.is_enabled() == enable {
        continue;
      }

      // Verify that all threads can be moved, before modifying any code
      for thread in suspended.threads() {
        hook.relocate(thread.pc(), enable)?;
      }

      // SAFETY: Unsuspended threads do not execute the patch (as guaranteed
      // by the caller), and suspended threads are moved below.
      unsafe { hook.write(enable, &protections[index])? };
      applied.push(index);

      for thread in suspended.threads() {
        if let Some(pc) = hook.relocate(thread.pc(), enable)? {
          thread.set_pc(pc);
        }
      }
    }
    suspended.apply()
  })();

  if result.is_err() {
    for &index in applied.iter().rev() {
      let (hook, enable) = operations[index];
      // SAFETY: Reverts code written above. It cannot have been modified
      // since, so this does not fail unless the system call does.
      let _ = unsafe { hook.write(!enable, &protections[index]) };
    }
  }

  // The threads are resumed before the vectors are released
  drop(suspended);
  result
}
