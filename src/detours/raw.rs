//! Untyped detours of raw pointers.

use crate::hook::Hook;
use crate::error::Result;

/// A raw detour.
///
/// # Dropping
///
/// Once dropped, the detour is disabled (without suspending other threads)
/// and its generated code is released. No other thread may execute the
/// target's prolog, or the trampoline, at
/// that moment. To disable a detour whilst threads are suspended, use a
/// [`Transaction`](crate::Transaction) before dropping it. If the detour
/// cannot be disabled (e.g. its target has been modified by a third party),
/// its generated code is leaked instead.
///
/// # Example
///
/// ```rust
/// # use detour::Result;
/// use detour::RawDetour;
/// use std::mem;
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
/// # fn main() -> Result<()> {
/// let mut hook = unsafe { RawDetour::new(add5 as *const (), add10 as *const ())? };
///
/// assert_eq!(add5(5), 10);
/// assert_eq!(hook.is_enabled(), false);
///
/// unsafe { hook.enable()? };
/// assert!(hook.is_enabled());
///
/// let original: fn(i32) -> i32 = unsafe { mem::transmute(hook.trampoline()) };
///
/// assert_eq!(add5(5), 15);
/// assert_eq!(original(5), 10);
///
/// unsafe { hook.disable()? };
/// assert_eq!(add5(5), 10);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct RawDetour(Hook);

impl RawDetour {
  /// Constructs a new inline detour patcher.
  ///
  /// The hook is disabled by default. Even when this function is successful,
  /// there is no guarantee that the detour function will actually get called
  /// when the target function gets called. An invocation of the target
  /// function might for example get inlined in which case it is impossible to
  /// hook at runtime.
  ///
  /// `target` need not be the entry of a function: it may be any instruction,
  /// e.g. for a mid-function detour (see the [`mid_function`] example). The
  /// detour is then jumped to with the state of the function at `target`, and
  /// must preserve it before continuing at the trampoline.
  ///
  /// [`mid_function`]: https://github.com/darfink/detour-rs/blob/master/examples/mid_function.rs
  ///
  /// # Safety
  ///
  /// `target` must point to the start of an instruction, and `detour` must be
  /// compatible with the state at `target`. If `target` is a function, the
  /// detour must be a function with a compatible signature and calling
  /// convention.
  ///
  /// The instructions overwritten by the jump to the detour (at least 5 bytes
  /// on x86, 4 or 16 bytes on AArch64, and 8 bytes on RISC-V) are moved to the
  /// trampoline. No other code may branch into them, other than to `target`
  /// itself.
  ///
  /// The scratch register of the architecture (X16 on AArch64, T1 on RISC-V)
  /// may be clobbered on the way to the detour, and from the trampoline back
  /// to the code following the overwritten instructions. It must therefore
  /// hold no live value at `target`, which is the case at the entry of a
  /// function. No other registers (nor flags) are modified.
  pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
    // SAFETY: Forwarded from the caller.
    unsafe { Hook::new(target, detour) }.map(RawDetour)
  }

  /// Enables the detour.
  ///
  /// # Safety
  ///
  /// The target must not be executing its prolog (i.e. the patched
  /// instructions) on another thread whilst the detour is being enabled.
  /// To suspend other threads meanwhile, use a [`Transaction`](crate::Transaction).
  pub unsafe fn enable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.0.enable() }
  }

  /// Disables the detour.
  ///
  /// # Safety
  ///
  /// See [`RawDetour::enable`].
  pub unsafe fn disable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.0.disable() }
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.0.is_enabled()
  }

  /// Returns a pointer to the trampoline, which invokes the original function
  /// (see [chaining](crate#detours-of-the-same-target)), regardless of whether
  /// the detour is enabled.
  ///
  /// The pointer is valid for as long as the detour exists.
  pub fn trampoline(&self) -> *const () {
    self.0.trampoline()
  }
}

impl crate::transaction::private::Sealed for RawDetour {
  fn hook(&self) -> Option<&Hook> {
    Some(&self.0)
  }
}

impl crate::Detour for RawDetour {}
