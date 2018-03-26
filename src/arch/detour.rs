use super::memory;
use error::{Error, Result};
use std::fmt;
use {alloc, arch, region, util};

/// An architecture-independent implementation of a base detour.
///
/// This class is never instantiated by itself, it merely exposes an API
/// available through it's descendants.
pub struct Detour {
  #[allow(dead_code)]
  relay: Option<alloc::ExecutableMemory>,
  trampoline: alloc::ExecutableMemory,
  patcher: arch::Patcher,
  enabled: bool,
}

impl Detour {
  pub(crate) unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
    if target == detour {
      Err(Error::SameAddress)?;
    }

    // Lock this so OS operations are not performed in parallell
    let mut pool = memory::POOL.lock().unwrap();

    if !util::is_executable_address(target)? || !util::is_executable_address(detour)? {
      Err(Error::NotExecutable)?;
    }

    // Create a trampoline generator for the target function
    let margin = arch::meta::prolog_margin(target);
    let trampoline = arch::Trampoline::new(target, margin)?;

    // A relay is used in case a normal branch cannot reach the destination
    let relay = if let Some(emitter) = arch::meta::relay_builder(target, detour)? {
      Some(memory::allocate_pic(&mut pool, &emitter, target)?)
    } else {
      None
    };

    // If a relay is supplied, use it instead of the detour address
    let detour = relay
      .as_ref()
      .map(|code| code.as_ptr() as *const ())
      .unwrap_or(detour);

    Ok(Detour {
      patcher: arch::Patcher::new(target, detour, trampoline.prolog_size())?,
      trampoline: memory::allocate_pic(&mut pool, trampoline.emitter(), target)?,
      enabled: false,
      relay,
    })
  }

  /// Enables or disables the detour.
  pub unsafe fn toggle(&mut self, enabled: bool) -> Result<()> {
    let _guard = memory::POOL.lock().unwrap();

    if self.enabled == enabled {
      return Ok(());
    }

    let mut region = {
      let area = self.patcher.area();
      region::View::new(area.as_ptr(), area.len())?
    };

    // Runtime code is by default only read-execute
    region
      .exec_with_prot(region::Protection::ReadWriteExecute, || {
        // Copy either the detour or the original bytes of the function
        self.patcher.toggle(enabled);
        self.enabled = enabled;
      })
      .map_err(|error| error.into())
  }

  /// Enables the detour.
  pub unsafe fn enable(&mut self) -> Result<()> {
    self.toggle(true)
  }

  /// Disables the detour.
  pub unsafe fn disable(&mut self) -> Result<()> {
    self.toggle(false)
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.enabled
  }

  /// Returns a reference to the generated trampoline.
  pub fn trampoline(&self) -> &() {
    unsafe {
      (self.trampoline.as_ptr() as *const ())
        .as_ref()
        .expect("trampoline should not be null")
    }
  }
}

impl Drop for Detour {
  /// Disables the detour, if enabled.
  fn drop(&mut self) {
    unsafe { self.disable().unwrap() };
  }
}

impl fmt::Debug for Detour {
  /// Output whether the detour is enabled or not.
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(
      f,
      "Detour {{ enabled: {}, trampoline: {:?} }}",
      self.is_enabled(),
      self.trampoline()
    )
  }
}

unsafe impl Send for Detour {}
unsafe impl Sync for Detour {}
