//! Detours of any instruction (e.g. mid-function detours), invoking a closure
//! with the registers.

use crate::error::Result;
use crate::hook::Hook;
use crate::memory::{self, CodeBlock};
use alloc::boxed::Box;
use core::fmt;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "aarch64")]
#[path = "aarch64.rs"]
mod arch;
#[cfg(target_arch = "riscv64")]
#[path = "riscv64.rs"]
mod arch;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[path = "x86.rs"]
mod arch;

#[cfg(test)]
mod tests;

pub use self::arch::Context;

/// The closure of a mid-function detour, and the number of its active
/// invocations (which keep the detour's generated code alive, see `Drop`).
struct Closure {
  handler: Box<dyn Fn(&mut Context) + Send + Sync>,
  active: AtomicUsize,
}

/// The addresses referenced by a shim.
struct Shim {
  /// The address of the shim itself.
  #[cfg_attr(not(target_arch = "x86"), allow(dead_code))]
  base: usize,
  /// The function invoking the handler (see [`arch::dispatch`]).
  dispatch: usize,
  /// The [`Closure`], passed to `dispatch`.
  handler: usize,
  /// The code continuing execution, i.e. the original instructions.
  trampoline: usize,
}

/// A detour of any instruction, invoking a closure with the registers.
///
/// Unlike the other detours, its target need not be a function: it may be
/// any instruction (e.g. within a loop). Whenever the instruction is reached,
/// the closure is called with the [`Context`]: the general-purpose registers
/// and flags at that point, which it may inspect and modify. Execution then
/// continues at the instruction, with the (modified) registers. All other
/// state is preserved, including the floating-point and SIMD registers (but
/// not the state of the RISC-V vector extension).
///
/// This is implemented by a generated *shim*, which is jumped to instead of
/// the instruction. It saves the state, calls the closure, restores the
/// state, and continues at the trampoline (the relocated instructions,
/// followed by a jump back). The stack pointer need not be aligned at the
/// target: the shim aligns the stack for the closure (and its context), and
/// skips the red zone of the target (on x86-64, and AArch64 on Apple
/// platforms).
///
/// The closure must not panic: since it is called from generated code, a
/// panic aborts the process. It is shared between threads, which may call it
/// concurrently.
///
/// The following state is not preserved, since Rust code does not use it
/// unless explicitly enabled: on x86, the AMX tiles (which are several KiB
/// large), and the vector registers if `XSAVE` is unsupported by the OS (only
/// those of SSE are saved, by `FXSAVE`); on AArch64, the SVE & SME state
/// beyond the Q registers (restoring them clears the upper lanes of the Z
/// registers); on RISC-V, the state of the vector extension. The code at the
/// target must therefore not hold live values in them.
///
/// # Dropping
///
/// Once dropped, the detour is disabled (without suspending other threads)
/// and its generated code and closure are released. No other thread may
/// execute the target's patched instructions, the shim, the closure, or the
/// trampoline, at that moment. To disable a detour whilst threads are
/// suspended, use a [`Transaction`](crate::Transaction) before dropping it.
/// If the detour cannot be disabled (e.g. its target has been modified by a
/// third party), or its closure is executing (e.g. the closure drops its own
/// detour), its generated code and closure are leaked instead.
///
/// # Example
///
/// ```rust
/// # #[cfg(target_arch = "x86_64")]
/// # fn main() -> detour::Result<()> {
/// use detour::ContextDetour;
///
/// #[unsafe(naked)]
/// extern "sysv64" fn double_sum(a: u64, b: u64) -> u64 {
///   core::arch::naked_asm!("mov rax, rdi", "add rax, rsi", "shl rax, 1", "ret")
/// }
///
/// // Detours the `add` instruction (3 bytes into the function)
/// let target = (double_sum as *const () as usize + 3) as *const ();
/// let detour = unsafe { ContextDetour::new(target, |context| context.rsi = 100)? };
///
/// assert_eq!(double_sum(2, 3), 10);
/// unsafe { detour.enable()? };
/// assert_eq!(double_sum(2, 3), 204);
/// # Ok(())
/// # }
/// # #[cfg(not(target_arch = "x86_64"))]
/// # fn main() {}
/// ```
#[doc(alias = "MidHook", alias = "MidDetour", alias = "mid-function")]
pub struct ContextDetour {
  // Dropped first, since it refers to the shim (which refers to the handler)
  hook: ManuallyDrop<Hook>,
  shim: ManuallyDrop<CodeBlock>,
  closure: ManuallyDrop<Box<Closure>>,
}

impl ContextDetour {
  /// Creates a new, disabled detour of the instruction at `target`, calling
  /// `handler` whenever it is reached.
  ///
  /// # Safety
  ///
  /// `target` must point to the start of an instruction. No other code may
  /// branch into the instructions overwritten by the jump to the shim (other
  /// than to `target` itself), and the scratch register of the architecture
  /// (X16 on AArch64, T1 on RISC-V) must hold no live value at `target`. See
  /// [`RawDetour::new`](crate::RawDetour::new) for details.
  ///
  /// Modifying the registers in `handler` must be valid for the code at
  /// `target`, which must not hold live values in state that is not
  /// preserved (see [`ContextDetour`]).
  pub unsafe fn new<F>(target: *const (), handler: F) -> Result<Self>
  where
    F: Fn(&mut Context) + Send + Sync + 'static,
  {
    let closure = Box::new(Closure {
      handler: Box::new(handler),
      active: AtomicUsize::new(0),
    });
    let shim = |base, trampoline| {
      arch::shim(&Shim {
        base,
        dispatch: arch::dispatch as *const () as usize,
        handler: &raw const *closure as usize,
        trampoline,
      })
    };
    let write = |block: &mut CodeBlock, code: &[u8]| {
      let _lock = memory::patch_lock();
      // SAFETY: The hook is disabled (or not yet created), so the shim cannot
      // be executing.
      unsafe { block.write(code) }
    };

    // The length of the shim does not depend on the addresses. It is written
    // before creating the hook, which requires it to be executable (on W^X
    // systems, a block becomes executable once written), and completed with
    // the address of the trampoline afterwards.
    let mut block = memory::allocate_near(target as usize, usize::MAX, shim(0, 0).len())?;
    let base = block.address();
    write(&mut block, &shim(base, 0))?;
    // SAFETY: Forwarded from the caller; the shim preserves the state.
    let hook = unsafe { Hook::new(target, block.as_ptr().cast())? };
    write(&mut block, &shim(base, hook.trampoline() as usize))?;

    Ok(ContextDetour {
      hook: ManuallyDrop::new(hook),
      shim: ManuallyDrop::new(block),
      closure: ManuallyDrop::new(closure),
    })
  }

  /// Enables the detour.
  ///
  /// # Safety
  ///
  /// The target must not be executing its patched instructions on another
  /// thread whilst the detour is being enabled. To suspend other threads
  /// meanwhile, use a [`Transaction`](crate::Transaction).
  pub unsafe fn enable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.hook.enable() }
  }

  /// Disables the detour.
  ///
  /// # Safety
  ///
  /// See [`ContextDetour::enable`].
  pub unsafe fn disable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.hook.disable() }
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.hook.is_enabled()
  }
}

impl Drop for ContextDetour {
  fn drop(&mut self) {
    // SAFETY: Restoring the original code is always valid.
    let disabled = unsafe { self.hook.disable() }.is_ok();

    // An executing closure (e.g. one dropping its own detour) returns to the
    // shim, which continues at the trampoline (owned by the hook)
    if disabled && self.closure.active.load(Ordering::SeqCst) == 0 {
      // SAFETY: Neither the target, other detours, nor an invocation refer to
      // the generated code (which is the only reference to the closure), and
      // the fields are never accessed again.
      unsafe {
        ManuallyDrop::drop(&mut self.hook);
        ManuallyDrop::drop(&mut self.shim);
        ManuallyDrop::drop(&mut self.closure);
      }
    }
    // Otherwise, the generated code and the closure are leaked
  }
}

impl fmt::Debug for ContextDetour {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ContextDetour")
      .field("hook", &*self.hook)
      .field("shim", &*self.shim)
      .finish_non_exhaustive()
  }
}

impl crate::transaction::private::Sealed for ContextDetour {
  fn hook(&self) -> Option<&Hook> {
    Some(&self.hook)
  }
}

impl crate::Detour for ContextDetour {}

/// Invokes the closure with the context saved by the shim.
///
/// # Safety
///
/// `context` must be valid for reads and writes, and `closure` must point to
/// the closure of a live detour.
unsafe fn invoke(context: *mut Context, closure: *const Closure) {
  // SAFETY: Guaranteed by the caller.
  let closure = unsafe { &*closure };
  closure.active.fetch_add(1, Ordering::SeqCst);
  // SAFETY: Guaranteed by the caller. A panic aborts the process (the shim's
  // callers are `extern`), so the count is always decremented.
  (closure.handler)(unsafe { &mut *context });
  closure.active.fetch_sub(1, Ordering::SeqCst);
}
