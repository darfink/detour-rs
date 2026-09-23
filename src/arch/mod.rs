//! Architecture-specific code generation.
//!
//! Each architecture exposes a `build` function, which disassembles a target,
//! generates a trampoline (the relocated prolog, followed by a jump back into
//! the original function), an optional relay (for detours beyond the reach of
//! a relative branch), and the bytes used to patch the target.

use crate::memory::CodeBlock;
use alloc::vec::Vec;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

#[cfg(target_arch = "aarch64")]
pub(crate) use self::aarch64::build;
#[cfg(all(test, target_arch = "aarch64"))]
pub(crate) use self::aarch64::FORCE_FAR;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) use self::x86::build;

// The AArch64 encoder is platform-independent, and tested on all 64-bit hosts
#[cfg(all(test, not(target_arch = "aarch64"), target_pointer_width = "64"))]
#[path = "aarch64.rs"]
#[allow(dead_code)]
mod aarch64_encoder;

/// The components of a detour, generated for a specific target.
pub(crate) struct Parts {
  /// The address at which the target is patched.
  pub patch_address: *mut u8,
  /// The original bytes at the patch address.
  pub original: Vec<u8>,
  /// The bytes redirecting the target to the detour (or relay).
  pub patched: Vec<u8>,
  /// Callable code, equivalent to the original target.
  pub trampoline: CodeBlock,
  /// The offsets of relocated instructions relative to the target, paired
  /// with the offset of their equivalent in the trampoline.
  ///
  /// Instructions whose equivalent is unknown (e.g. rewritten by the x86
  /// block encoder) are omitted.
  pub relocated: Vec<(u32, u32)>,
  /// An intermediate jump to the detour, if it is out of reach.
  pub relay: Option<CodeBlock>,
}
