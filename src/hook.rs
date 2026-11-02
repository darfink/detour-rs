//! The architecture-independent implementation of a detour.
//!
//! The detours of a target share a site: its original code, and a trampoline.
//! The enabled detours of a site form a chain, in the order they were enabled.
//! The target branches to the last one, whose original function (its link)
//! branches to the one enabled before it, and so on, until the trampoline is
//! reached. Each link is an indirect jump through a pointer, so detours can be
//! enabled and disabled in any order.

use crate::arch;
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock, Protections};
use crate::thread::Threads;
use crate::transaction;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::mem::ManuallyDrop;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// All sites with detours, guarded by the global patch lock.
pub(crate) struct Registry {
  sites: Vec<Site>,
}

// SAFETY: The links of a site are only accessed whilst the lock is held, and
// remain valid whilst they are part of a chain (see `Site::chain`).
unsafe impl Send for Registry {}

impl Registry {
  pub const fn new() -> Self {
    Registry { sites: Vec::new() }
  }

  /// Returns the site of `target`, which must exist.
  fn site(&mut self, target: usize) -> &mut Site {
    self
      .sites
      .iter_mut()
      .find(|site| site.target == target)
      .expect("the site of a hook exists")
  }

  /// Reserves capacity for enabling `count` more detours of `hook`'s target,
  /// so that no allocation is required whilst threads are suspended.
  pub(crate) fn reserve(&mut self, hook: &Hook, count: usize) {
    self.site(hook.target).chain.reserve(count);
  }
}

/// The detours of a target.
struct Site {
  target: usize,
  /// See [`arch::Site`].
  code: arch::Site,
  /// The links of the enabled detours, in the order they were enabled. They
  /// are owned by their hooks, which remove them before being dropped.
  chain: Vec<*const Link>,
  /// The number of hooks of the site.
  hooks: usize,
}

/// The part of a hook referenced by its site (boxed, so its address is
/// stable).
struct Link {
  /// The detour, i.e. the destination of the link enabled after this one.
  detour: usize,
  /// The bytes redirecting the target to the detour (or its relay).
  patched: Box<[u8]>,
  /// The destination of the link: the next detour of the chain, or the
  /// trampoline.
  next: AtomicUsize,
}

/// How enabling or disabling a hook changes the target's code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
  /// The original code is replaced (the first detour is enabled).
  Patch,
  /// The original code is restored (the last detour is disabled).
  Restore,
  /// The target is redirected to another detour; threads are unaffected.
  Redirect,
  /// Only links are updated.
  Relink,
}

/// How to revert an applied operation (see [`Hook::revert`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Undo {
  /// The hook was enabled.
  Enabled,
  /// The hook was disabled, from `position` of the chain.
  Disabled { position: usize },
}

/// The building block of all detour types.
pub struct Hook {
  target: usize,
  patch_address: *mut u8,
  /// The original code of the target.
  original: Box<[u8]>,
  /// See [`arch::Site::relocated`].
  relocated: Box<[(u32, u32)]>,
  /// The site's trampoline.
  trampoline: usize,
  // Released manually, since the target or other detours may still refer to
  // them
  link: ManuallyDrop<Box<Link>>,
  stub: ManuallyDrop<CodeBlock>,
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

    let mut registry = memory::patch_lock();

    if !memory::is_executable(target)? || !memory::is_executable(detour)? {
      return Err(Error::NotExecutable);
    }

    // Detours of the same target share its site (and original code)
    let existing = registry.sites.iter().position(|site| site.target == target as usize);
    let (new, redirect) = match existing {
      Some(index) => {
        let code = &registry.sites[index].code;
        (None, arch::redirect(code.patch_address, code.layout, detour)?)
      },
      None => {
        // SAFETY: The target has been verified to be executable.
        let (site, redirect) = unsafe { arch::build(target, detour)? };
        (Some(site), redirect)
      },
    };
    let code = new.as_ref().unwrap_or_else(|| &registry.sites[existing.unwrap()].code);

    let link = Box::new(Link {
      detour: detour as usize,
      patched: redirect.patched.into_boxed_slice(),
      next: AtomicUsize::new(code.trampoline.address()),
    });
    let stub_code = arch::link(link.next.as_ptr());
    let mut stub = memory::allocate_near(target as usize, usize::MAX, stub_code.len())?;
    // SAFETY: The stub has just been allocated, and cannot be executing.
    unsafe { stub.write(&stub_code)? };

    let hook = Hook {
      target: target as usize,
      patch_address: code.patch_address,
      original: code.original.clone().into_boxed_slice(),
      relocated: code.relocated.clone().into_boxed_slice(),
      trampoline: code.trampoline.address(),
      link: ManuallyDrop::new(link),
      stub: ManuallyDrop::new(stub),
      relay: ManuallyDrop::new(redirect.relay),
      enabled: AtomicBool::new(false),
    };

    match (new, existing) {
      (Some(code), _) => registry.sites.push(Site {
        target: target as usize,
        code,
        chain: Vec::new(),
        hooks: 1,
      }),
      (None, Some(index)) => registry.sites[index].hooks += 1,
      (None, None) => unreachable!(),
    }
    Ok(hook)
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

  /// Returns a pointer to the hook's original function: the detour enabled
  /// before it (of the same target), or the trampoline.
  pub fn trampoline(&self) -> *const () {
    self.stub.as_ptr().cast()
  }

  /// Returns the range of patched bytes.
  fn patch_range(&self) -> Range<usize> {
    let start = self.patch_address as usize;
    start..start + self.original.len()
  }

  /// Queries the protection of the patched bytes (see [`Hook::apply`]).
  pub(crate) fn protections(&self) -> Result<Protections> {
    Protections::query(self.patch_address, self.original.len())
  }

  /// Returns how enabling or disabling the hook changes the target's code.
  pub(crate) fn change(&self, registry: &mut Registry, enable: bool) -> Change {
    let chain = &registry.site(self.target).chain;
    let is_last = chain.last() == Some(&(&**self.link as *const Link));
    match (enable, chain.len()) {
      (true, 0) => Change::Patch,
      (true, _) => Change::Redirect,
      (false, 1) => Change::Restore,
      (false, _) if is_last => Change::Redirect,
      (false, _) => Change::Relink,
    }
  }

  /// Enables or disables the hook, returning how to revert it if applied.
  ///
  /// The operation may be applied despite an error (e.g. if the code was
  /// written, but its protection could not be restored). The caller must hold
  /// the global patch lock, and must not allocate if threads are suspended
  /// (see [`Registry::reserve`]).
  ///
  /// # Safety
  ///
  /// No thread may execute the patched bytes whilst they are written.
  pub(crate) unsafe fn apply(
    &self,
    registry: &mut Registry,
    enable: bool,
    protections: &Protections,
  ) -> (Option<Undo>, Result<()>) {
    let site = registry.site(self.target);
    let link: *const Link = &**self.link;
    let trampoline = site.code.trampoline.address();

    if enable {
      // SAFETY: Links are valid whilst chained.
      let last = site.chain.last().map(|&last| unsafe { &*last });
      // Calls to the original function are forwarded before the target is
      // redirected
      self.link.next.store(last.map_or(trampoline, |last| last.detour), Ordering::SeqCst);

      let expected = last.map_or(&*self.original, |last| &*last.patched);
      // SAFETY: Forwarded from the caller.
      let (written, result) = unsafe { self.replace(expected, &self.link.patched, protections) };
      if !written {
        self.link.next.store(trampoline, Ordering::SeqCst);
        return (None, result);
      }
      site.chain.push(link);
      self.enabled.store(true, Ordering::SeqCst);
      return (Some(Undo::Enabled), result);
    }

    let position =
      site.chain.iter().position(|&other| other == link).expect("an enabled hook is chained");
    let next = self.link.next.load(Ordering::SeqCst);
    let mut result = Ok(());

    if position + 1 == site.chain.len() {
      // The target is redirected to the previous detour (or restored)
      let previous = position.checked_sub(1).map(|index| site.chain[index]);
      // SAFETY: Links are valid whilst chained.
      let replacement = previous.map_or(&*self.original, |previous| unsafe { &*(*previous).patched });
      let written;
      // SAFETY: Forwarded from the caller.
      (written, result) = unsafe { self.replace(&self.link.patched, replacement, protections) };
      if !written {
        return (None, result);
      }
      site.chain.pop();
    } else {
      // The detour enabled after this one bypasses it
      // SAFETY: Links are valid whilst chained.
      unsafe { &*site.chain[position + 1] }.next.store(next, Ordering::SeqCst);
      site.chain.remove(position);
    }

    self.link.next.store(trampoline, Ordering::SeqCst);
    self.enabled.store(false, Ordering::SeqCst);
    (Some(Undo::Disabled { position }), result)
  }

  /// Reverts an operation applied by [`Hook::apply`]. Operations are
  /// reverted in reverse order, and errors are ignored (as are by a failing
  /// transaction). A revert may fail as well, so the chain is not assumed to
  /// be in its state prior to the operation.
  ///
  /// # Safety
  ///
  /// See [`Hook::apply`].
  pub(crate) unsafe fn revert(&self, registry: &mut Registry, undo: Undo, protections: &Protections) {
    let Undo::Disabled { position } = undo else {
      // Unless the revert of a subsequent operation (disabling it) failed
      if self.is_enabled() {
        // SAFETY: Forwarded from the caller.
        let _ = unsafe { self.apply(registry, false, protections) };
      }
      return;
    };

    let site = registry.site(self.target);
    let link: *const Link = &**self.link;
    let trampoline = site.code.trampoline.address();
    // Fewer detours remain if reverting a preceding operation failed
    let position = position.min(site.chain.len());
    // SAFETY: Links are valid whilst chained.
    let previous = position.checked_sub(1).map(|index| unsafe { &*site.chain[index] });
    self.link.next.store(previous.map_or(trampoline, |previous| previous.detour), Ordering::SeqCst);

    if position == site.chain.len() {
      let expected = previous.map_or(&*self.original, |previous| &*previous.patched);
      // SAFETY: Forwarded from the caller.
      let (written, _) = unsafe { self.replace(expected, &self.link.patched, protections) };
      if !written {
        self.link.next.store(trampoline, Ordering::SeqCst);
        return;
      }
    } else {
      // SAFETY: Links are valid whilst chained.
      unsafe { &*site.chain[position] }.next.store(self.link.detour, Ordering::SeqCst);
    }

    // The capacity remains from the removal, so this does not allocate
    site.chain.insert(position, link);
    self.enabled.store(true, Ordering::SeqCst);
  }

  /// Replaces `expected` with `replacement` at the patch address, returning
  /// whether `replacement` was written (possibly despite an error).
  ///
  /// # Safety
  ///
  /// See [`Hook::apply`].
  unsafe fn replace(&self, expected: &[u8], replacement: &[u8], protections: &Protections) -> (bool, Result<()>) {
    // Refuse to overwrite code that has been modified by someone else
    // SAFETY: The patch area is readable (it was read upon creation).
    let current = || unsafe { core::slice::from_raw_parts(self.patch_address, expected.len()) };
    if current() != expected {
      return (false, Err(Error::TargetModified));
    }

    // SAFETY: The replacement is either the original code, or a branch to a
    // detour that has been verified during creation.
    let result = unsafe { memory::patch_code(self.patch_address, replacement, protections) };
    // The code may have been written despite an error (e.g. if the protection
    // could not be restored), in which case the state must follow the code.
    // Otherwise the trampoline could be released whilst still in use.
    (current() == replacement, result)
  }

  /// Returns where a suspended thread at `pc` must resume, once the target's
  /// code is patched or restored (see [`Change`]), or `None` if it is
  /// unaffected.
  ///
  /// - Patching: a thread executing the instructions that are overwritten is
  ///   moved to their equivalent in the trampoline.
  /// - Restoring: a thread in the midst of the patch (having executed part of
  ///   it, e.g. the short jump of a hot patch) is moved to the start of the
  ///   target. The trampoline remains valid, so threads executing it are
  ///   unaffected.
  pub(crate) fn relocate(&self, pc: usize, change: Change) -> Result<Option<usize>> {
    let patch = self.patch_range();
    if !patch.contains(&pc) {
      return Ok(None);
    }

    match change {
      Change::Patch => {
        // The padding preceding a hot-patched function is never executed
        if pc < self.target {
          return Ok(None);
        }
        let offset = (pc - self.target) as u32;
        self
          .relocated
          .iter()
          .find(|(original, _)| *original == offset)
          .map(|&(_, relocated)| Some(self.trampoline + relocated as usize))
          .ok_or(Error::ThreadNotRelocatable)
      },
      // The thread has yet to execute the patch; the original is restored
      Change::Restore if pc == patch.start.max(self.target) => Ok(None),
      // Any executed part of the patch (e.g. `AUTIASP`) is redone by the
      // target's prolog (e.g. `PACIASP`)
      Change::Restore => Ok(Some(self.target)),
      // Patches of the same target have the same layout, so a thread in the
      // midst of one continues with the other
      Change::Redirect | Change::Relink => Ok(None),
    }
  }
}

impl Drop for Hook {
  /// Disables the hook, if enabled.
  fn drop(&mut self) {
    // SAFETY: Restoring the original code is always valid.
    if unsafe { self.disable() }.is_err() {
      // The target or other detours may still branch to the generated code,
      // and the chain refers to the link, so they are intentionally leaked
      // (along with the site).
      return;
    }

    let mut registry = memory::patch_lock();
    let index = registry
      .sites
      .iter()
      .position(|site| site.target == self.target)
      .expect("the site of a hook exists");
    registry.sites[index].hooks -= 1;
    let site = (registry.sites[index].hooks == 0).then(|| registry.sites.swap_remove(index));
    drop(registry);
    drop(site);

    // SAFETY: Neither the target nor other detours refer to the generated
    // code, and the fields are never accessed again.
    unsafe {
      ManuallyDrop::drop(&mut self.link);
      ManuallyDrop::drop(&mut self.stub);
      ManuallyDrop::drop(&mut self.relay);
    }
  }
}

impl fmt::Debug for Hook {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Hook")
      .field("target", &(self.target as *const ()))
      .field("detour", &(self.link.detour as *const ()))
      .field("trampoline", &self.trampoline())
      .field("enabled", &self.is_enabled())
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::Change;
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
    let (target, trampoline) = (ret5 as *const () as usize, hook.trampoline);

    // Enabling moves threads executing the overwritten instructions
    assert_eq!(hook.relocate(target, Change::Patch), Ok(Some(trampoline)));
    assert_eq!(hook.relocate(target + 1, Change::Patch), Ok(Some(trampoline + 1)));
    assert_eq!(hook.relocate(target + 2, Change::Patch), Err(crate::Error::ThreadNotRelocatable));
    assert_eq!(hook.relocate(target + 5, Change::Patch), Ok(None));
    assert_eq!(hook.relocate(target - 1, Change::Patch), Ok(None));

    // Disabling only moves threads in the midst of the patch
    assert_eq!(hook.relocate(target, Change::Restore), Ok(None));
    assert_eq!(hook.relocate(target + 1, Change::Restore), Ok(Some(target)));
    assert_eq!(hook.relocate(trampoline + 1, Change::Restore), Ok(None));
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
    let (target, trampoline) = (target as *const () as usize, hook.trampoline);

    // The padding is never executed, and only the first instruction is patched
    assert_eq!(hook.relocate(target - 5, Change::Patch), Ok(None));
    assert_eq!(hook.relocate(target, Change::Patch), Ok(Some(trampoline)));
    assert_eq!(hook.relocate(target + 2, Change::Patch), Ok(None));

    // A thread having executed the short jump restarts at the target
    assert_eq!(hook.relocate(target - 5, Change::Restore), Ok(Some(target)));
    assert_eq!(hook.relocate(target, Change::Restore), Ok(None));
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
    let (target, trampoline) = (ret5 as *const () as usize, hook.trampoline);

    // A single instruction is patched
    assert_eq!(hook.relocate(target, Change::Patch), Ok(Some(trampoline)));
    assert_eq!(hook.relocate(target + 4, Change::Patch), Ok(None));
    assert_eq!(hook.relocate(target, Change::Restore), Ok(None));
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
    let (target, trampoline) = (add5 as *const () as usize, hook.trampoline);

    // Four instructions are replaced, each relocated to a single instruction
    assert_eq!(hook.relocate(target + 8, Change::Patch), Ok(Some(trampoline + 8)));
    assert_eq!(hook.relocate(target + 12, Change::Patch), Ok(Some(trampoline + 12)));
    assert_eq!(hook.relocate(target + 16, Change::Patch), Ok(None));

    // A thread in the midst of the absolute jump restarts at the target
    assert_eq!(hook.relocate(target, Change::Restore), Ok(None));
    assert_eq!(hook.relocate(target + 4, Change::Restore), Ok(Some(target)));
    Ok(())
  }

  /// Returns `5`.
  #[unsafe(naked)]
  unsafe extern "C" fn ret5() -> i32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    naked_asm!("mov eax, 5", "ret");
    #[cfg(target_arch = "aarch64")]
    naked_asm!("mov w0, #5", "ret");
  }

  /// See [`ret5`] (each test detours its own target).
  #[unsafe(naked)]
  unsafe extern "C" fn ret6() -> i32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    naked_asm!("mov eax, 6", "ret");
    #[cfg(target_arch = "aarch64")]
    naked_asm!("mov w0, #6", "ret");
  }

  #[test]
  fn state_follows_code_written_despite_errors() -> Result<()> {
    let detour = detour(ret5)?;
    let hook = detour.hook().unwrap();
    let mut registry = crate::memory::patch_lock();
    let protections = hook.protections()?;

    for enable in [true, false] {
      crate::memory::FAIL_AFTER_WRITE.set(Some(0));
      // SAFETY: No other thread is executing `ret5`.
      let (undo, result) = unsafe { hook.apply(&mut registry, enable, &protections) };
      crate::memory::FAIL_AFTER_WRITE.set(None);

      // The code was written, so the state is updated nonetheless
      assert!(undo.is_some() && result.is_err());
      assert_eq!(hook.is_enabled(), enable);
      // SAFETY: `ret5` has no preconditions.
      assert_eq!(unsafe { ret5() }, if enable { 10 } else { 5 });
    }
    Ok(())
  }

  #[test]
  fn transactions_revert_code_written_despite_errors() -> Result<()> {
    let detour = detour(ret6)?;
    let mut transaction = crate::Transaction::new();
    transaction.enable(&detour);

    crate::memory::FAIL_AFTER_WRITE.set(Some(0));
    // SAFETY: No other thread is executing `ret6`.
    let result = unsafe { transaction.commit(crate::Threads::None) };
    crate::memory::FAIL_AFTER_WRITE.set(None);

    // Both the write and its revert fail, but the code is reverted
    assert!(result.is_err());
    assert!(!detour.is_enabled());
    // SAFETY: `ret6` has no preconditions.
    assert_eq!(unsafe { ret6() }, 6);
    Ok(())
  }
}
