//! Architecture-specific code generation.
//!
//! Each architecture prepares a [`Site`] for a target, by disassembling it and
//! generating a trampoline (the relocated prolog, followed by a jump back into
//! the original function). For each detour of the site, a [`Redirect`] is
//! generated: the bytes patching the target, and an optional relay (for
//! detours beyond the reach of a relative branch). Each detour also has a
//! link: an indirect jump through a pointer, to the next detour of the chain
//! or to the trampoline.

use crate::memory::CodeBlock;
use alloc::vec::Vec;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

#[cfg(target_arch = "aarch64")]
pub(crate) use self::aarch64::{Layout, build, link, redirect};
#[cfg(all(test, target_arch = "aarch64"))]
pub(crate) use self::aarch64::FORCE_FAR;
#[cfg(target_arch = "riscv64")]
pub(crate) use self::riscv64::{Layout, build, link, redirect};
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) use self::x86::{Layout, build, link, redirect};

// The AArch64 encoder is platform-independent, and tested on all 64-bit hosts
#[cfg(all(test, not(target_arch = "aarch64"), target_pointer_width = "64"))]
#[path = "aarch64.rs"]
#[allow(dead_code)]
mod aarch64_encoder;

// Likewise, the RISC-V encoder (only the encoder; the rest requires the target)
#[cfg(all(test, not(target_arch = "riscv64"), target_pointer_width = "64"))]
#[path = "riscv64.rs"]
#[allow(dead_code)]
mod riscv64_encoder;

/// The code of a target, shared by all of its detours.
pub(crate) struct Site {
  /// The address at which the target is patched.
  pub patch_address: *mut u8,
  /// The original bytes at the patch address.
  pub original: Vec<u8>,
  /// Callable code, equivalent to the original target.
  pub trampoline: CodeBlock,
  /// The offsets of relocated instructions relative to the target, paired
  /// with the offset of their equivalent in the trampoline.
  ///
  /// Instructions whose equivalent is unknown (e.g. rewritten by the x86
  /// block encoder) are omitted.
  pub relocated: Vec<(u32, u32)>,
  /// How the target is patched, which is the same for all of its detours.
  pub layout: Layout,
}

/// The redirection of a target to a specific detour.
pub(crate) struct Redirect {
  /// The bytes redirecting the target to the detour (or relay).
  pub patched: Vec<u8>,
  /// An intermediate jump to the detour, if it is out of reach.
  pub relay: Option<CodeBlock>,
}
