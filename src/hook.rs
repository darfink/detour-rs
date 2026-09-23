//! The architecture-independent implementation of a detour.

use crate::arch;
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock, Protections};
use crate::thread::Threads;
use crate::transaction;
use alloc::boxed::Box;
use core::fmt;
use core::mem::ManuallyDrop;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

/// The building block of all detour types.
pub struct Hook {
  target: usize,
  detour: *const (),
  patch_address: *mut u8,
  original: Box<[u8]>,
  patched: Box<[u8]>,
  /// See [`arch::Parts::relocated`].
  relocated: Box<[(u32, u32)]>,
  // Released manually, since the target may still refer to them
  trampoline: ManuallyDrop<CodeBlock>,
  relay: ManuallyDrop<Option<CodeBlock>>,
  enabled: AtomicBool,
}

// SAFETY: The raw pointers refer to code, which is not tied to a thread, and
// all modifications of it are serialized by the global patch lock.
unsafe impl Send for Hook {}
// SAFETY: See above; `&Hook` only exposes atomic state and locked operations.
unsafe impl Sync for Hook {}

impl Hook {
  /// Creates a new, disabled hook.
  ///
  /// # Safety
  ///
  /// `target` must point to a function, and `detour` to a function compatible
  /// with the target's calling convention and signature.
  pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
    if target == detour {
      return Err(Error::SameAddress);
    }

    let _lock = memory::patch_lock();

    if !memory::is_executable(target)? || !memory::is_executable(detour)? {
      return Err(Error::NotExecutable);
    }

    // SAFETY: The target has been verified to be executable.
    let parts = unsafe { arch::build(target, detour)? };

    Ok(Hook {
      target: target as usize,
      detour,
      patch_address: parts.patch_address,
      original: parts.original.into_boxed_slice(),
      patched: parts.patched.into_boxed_slice(),
      relocated: parts.relocated.into_boxed_slice(),
      trampoline: ManuallyDrop::new(parts.trampoline),
      relay: ManuallyDrop::new(parts.relay),
      enabled: AtomicBool::new(false),
    })
  }

  /// Enables the hook.
  pub unsafe fn enable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { transaction::commit(&[(self, true)], Threads::None) }
  }

  /// Disables the hook.
  pub unsafe fn disable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { transaction::commit(&[(self, false)], Threads::None) }
  }

  /// Returns whether the hook is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.enabled.load(Ordering::SeqCst)
  }

  /// Returns a pointer to the trampoline (i.e. the callable original).
  pub fn trampoline(&self) -> *const () {
    self.trampoline.as_ptr().cast()
  }

  /// Returns the range of patched bytes.
  fn patch_range(&self) -> Range<usize> {
    let start = self.patch_address as usize;
    start..start + self.patched.len()
  }

  /// Queries the protection of the patched bytes (see [`Hook::write`]).
  pub(crate) fn protections(&self) -> Result<Protections> {
    Protections::query(self.patch_address, self.patched.len())
  }

  /// Writes the code of the enabled or disabled state, without allocating.
  ///
  /// The caller must hold the global patch lock.
  ///
  /// # Safety
  ///
  /// No thread may execute the patched bytes whilst they are written.
  pub(crate) unsafe fn write(&self, enable: bool, protections: &Protections) -> Result<()> {
    let (expected, replacement) = if enable {
      (&self.original, &self.patched)
    } else {
      (&self.patched, &self.original)
    };

    // Refuse to overwrite code that has been modified by someone else, e.g.
    // another detour of the same target that was enabled after this one.
    // SAFETY: The patch area is readable (it was read upon creation).
    let current = unsafe { core::slice::from_raw_parts(self.patch_address, expected.len()) };
    if current != &expected[..] {
      return Err(Error::TargetModified);
    }

    // SAFETY: The replacement is either the original code, or a branch to a
    // detour that has been verified during creation.
    unsafe { memory::patch_code(self.patch_address, replacement, protections)? };
    self.enabled.store(enable, Ordering::SeqCst);
    Ok(())
  }

  /// Returns where a suspended thread at `pc` must resume, once the hook is
  /// enabled or disabled, or `None` if it is unaffected.
  ///
  /// - Enabling: a thread executing the instructions that are overwritten is
  ///   moved to their equivalent in the trampoline.
  /// - Disabling: a thread in the midst of the patch (having executed part of
  ///   it, e.g. the short jump of a hot patch) is moved to the start of the
  ///   target. The trampoline remains valid, so threads executing it are
  ///   unaffected.
  pub(crate) fn relocate(&self, pc: usize, enable: bool) -> Result<Option<usize>> {
    let patch = self.patch_range();
    if !patch.contains(&pc) {
      return Ok(None);
    }

    if enable {
      // The padding preceding a hot-patched function is never executed
      if pc < self.target {
        return Ok(None);
      }
      let offset = (pc - self.target) as u32;
      self
        .relocated
        .iter()
        .find(|(original, _)| *original == offset)
        .map(|&(_, relocated)| Some(self.trampoline.address() + relocated as usize))
        .ok_or(Error::ThreadNotRelocatable)
    } else if pc == patch.start.max(self.target) {
      // The thread has yet to execute the patch; the original is restored
      Ok(None)
    } else {
      // Any executed part of the patch (e.g. `AUTIASP`) is redone by the
      // target's prolog (e.g. `PACIASP`)
      Ok(Some(self.target))
    }
  }
}

impl Drop for Hook {
  /// Disables the hook, if enabled.
  fn drop(&mut self) {
    // SAFETY: Restoring the original code is always valid.
    if unsafe { self.disable() }.is_ok() {
      // SAFETY: The target no longer refers to the generated code, and the
      // fields are never accessed again.
      unsafe {
        ManuallyDrop::drop(&mut self.trampoline);
        ManuallyDrop::drop(&mut self.relay);
      }
    }
    // Otherwise the target may still branch to the trampoline or relay, so
    // their memory is intentionally leaked.
  }
}

impl fmt::Debug for Hook {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Hook")
      .field("target", &(self.target as *const ()))
      .field("detour", &self.detour)
      .field("trampoline", &self.trampoline())
      .field("enabled", &self.is_enabled())
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use crate::transaction::private::Sealed;
  use crate::{RawDetour, Result};
  use core::arch::naked_asm;

  extern "C" fn ret10() -> i32 {
    10
  }

  fn detour(target: unsafe extern "C" fn() -> i32) -> Result<RawDetour> {
    // SAFETY: Both functions share the same signature.
    unsafe { RawDetour::new(target as *const (), ret10 as *const ()) }
  }

  #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
  #[test]
  fn relocates_program_counters() -> Result<()> {
    // Instructions at offsets 0, 1 & 4 (x86-64), or 0, 1 & 3 (x86)
    #[unsafe(naked)]
    unsafe extern "C" fn ret5() -> i32 {
      #[cfg(target_arch = "x86_64")]
      naked_asm!("push rbp", "mov rbp, rsp", "mov eax, 5", "pop rbp", "ret");
      #[cfg(target_arch = "x86")]
      naked_asm!("push ebp", "mov ebp, esp", "mov eax, 5", "pop ebp", "ret");
    }

    let detour = detour(ret5)?;
    let hook = detour.hook().unwrap();
    let (target, trampoline) = (ret5 as *const () as usize, hook.trampoline() as usize);

    // Enabling moves threads executing the overwritten instructions
    assert_eq!(hook.relocate(target, true), Ok(Some(trampoline)));
    assert_eq!(hook.relocate(target + 1, true), Ok(Some(trampoline + 1)));
    assert_eq!(hook.relocate(target + 2, true), Err(crate::Error::ThreadNotRelocatable));
    assert_eq!(hook.relocate(target + 5, true), Ok(None));
    assert_eq!(hook.relocate(target - 1, true), Ok(None));

    // Disabling only moves threads in the midst of the patch
    assert_eq!(hook.relocate(target, false), Ok(None));
    assert_eq!(hook.relocate(target + 1, false), Ok(Some(target)));
    assert_eq!(hook.relocate(trampoline + 1, false), Ok(None));
    Ok(())
  }

  #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
  #[test]
  fn hot_patch_relocates_program_counters() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn hot_ret5() -> i32 {
      naked_asm!(
        "nop", "nop", "nop", "nop", "nop",
        "xor eax, eax",
        "ret",
        "mov eax, 5",
      )
    }

    let target = hot_ret5 as *const () as usize + 5;
    // SAFETY: The offset is the entry point of the fixture.
    let target: unsafe extern "C" fn() -> i32 = unsafe { core::mem::transmute(target) };
    let detour = detour(target)?;
    let hook = detour.hook().unwrap();
    let (target, trampoline) = (target as *const () as usize, hook.trampoline() as usize);

    // The padding is never executed, and only the first instruction is patched
    assert_eq!(hook.relocate(target - 5, true), Ok(None));
    assert_eq!(hook.relocate(target, true), Ok(Some(trampoline)));
    assert_eq!(hook.relocate(target + 2, true), Ok(None));

    // A thread having executed the short jump restarts at the target
    assert_eq!(hook.relocate(target - 5, false), Ok(Some(target)));
    assert_eq!(hook.relocate(target, false), Ok(None));
    Ok(())
  }

  #[cfg(target_arch = "aarch64")]
  #[test]
  fn relocates_program_counters() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn ret5() -> i32 {
      naked_asm!("mov w0, #5", "ret")
    }

    let detour = detour(ret5)?;
    let hook = detour.hook().unwrap();
    let (target, trampoline) = (ret5 as *const () as usize, hook.trampoline() as usize);

    // A single instruction is patched
    assert_eq!(hook.relocate(target, true), Ok(Some(trampoline)));
    assert_eq!(hook.relocate(target + 4, true), Ok(None));
    assert_eq!(hook.relocate(target, false), Ok(None));
    Ok(())
  }

  #[cfg(target_arch = "aarch64")]
  #[test]
  fn absolute_patch_relocates_program_counters() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn add5() -> i32 {
      naked_asm!(
        "mov w0, #1",
        "add w0, w0, #1",
        "add w0, w0, #1",
        "add w0, w0, #1",
        "add w0, w0, #1",
        "ret",
      )
    }

    crate::arch::FORCE_FAR.set(true);
    let detour = detour(add5);
    crate::arch::FORCE_FAR.set(false);
    let detour = detour?;
    let hook = detour.hook().unwrap();
    let (target, trampoline) = (add5 as *const () as usize, hook.trampoline() as usize);

    // Four instructions are replaced, each relocated to a single instruction
    assert_eq!(hook.relocate(target + 8, true), Ok(Some(trampoline + 8)));
    assert_eq!(hook.relocate(target + 12, true), Ok(Some(trampoline + 12)));
    assert_eq!(hook.relocate(target + 16, true), Ok(None));

    // A thread in the midst of the absolute jump restarts at the target
    assert_eq!(hook.relocate(target, false), Ok(None));
    assert_eq!(hook.relocate(target + 4, false), Ok(Some(target)));
    Ok(())
  }
}
