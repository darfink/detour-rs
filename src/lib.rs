//! A cross-platform detour library written in Rust.
//!
//! ## Intro
//!
//! This library provides a thread-safe, inline detouring functionality by
//! disassembling and patching functions during runtime, using assembly opcodes
//! allocated within executable memory. It modifies the target functions and
//! replaces their prolog with an unconditional jump.
//!
//! Beyond the basic functionality this library handles several different edge
//! cases:
//!
//! - Relative branches (including `loop`/`jrcxz` and branches within the
//!   prolog).
//! - RIP-relative operands (x86-64) and PC-relative instructions (AArch64).
//! - Detects NOP-padding.
//! - Relay for large offsets (>2 GiB on x86-64, >128 MiB on AArch64).
//! - Supports hot patching.
//! - Preserves BTI/PAC landing pads (AArch64).
//!
//! ## Detours
//!
//! Three different types of detours are provided:
//!
//! - [Static](./struct.StaticDetour.html): A static & type-safe interface.
//!   Thanks to its static nature it can accept a closure as its detour, but is
//!   required to be statically defined at compile time.
//!
//! - [Generic](./struct.GenericDetour.html): A type-safe interface — the same
//!   prototype is enforced for both the target and the detour. It is also
//!   enforced when invoking the original target.
//!
//! - [Raw](./struct.RawDetour.html): The underlying building block that the
//!   others types abstract upon. It has no type-safety and interacts with raw
//!   pointers. It should be avoided unless any types are references, or not
//!   known until runtime.
//!
//! ## Platforms
//!
//! - Architectures: `x86`, `x86-64` & `AArch64`.
//! - Operating systems: Windows, Linux, macOS, and other Unix-like systems.
//!
//! On macOS, code signing prevents making `__TEXT` writable; code is instead
//! patched using copy-on-write or remapping of the affected pages. Executable
//! memory is allocated with `MAP_JIT` on Apple silicon. Systems enforcing W^X
//! are supported, as long as code pages may be re-protected.
//!
//! ## Procedure
//!
//! To illustrate a detour on an x86 platform:
//!
//! ```c
//! 0 int return_five() {
//! 1     return 5;
//! 00400020 [b8 05 00 00 00] mov eax, 5
//! 00400025 [c3]             ret
//! 2 }
//! 3
//! 4 int detour_function() {
//! 5     return 10;
//! 00400040 [b8 0A 00 00 00] mov eax, 10
//! 00400045 [c3]             ret
//! 6 }
//! ```
//!
//! To detour `return_five` the library by default tries to replace five bytes
//! with a relative jump (the optimal scenario), which works in this case.
//! Executable memory will be allocated for the instruction and the function's
//! prolog will be replaced.
//!
//! ```c
//! 0 int return_five() {
//! 1     return detour_function();
//! 00400020 [e9 16 00 00 00] jmp 1b <detour_function>
//! 00400025 [c3]             ret
//! 2 }
//! 3
//! 4 int detour_function() {
//! 5     return 10;
//! 00400040 [b8 0a 00 00 00] mov eax, 10
//! 00400045 [c3]             ret
//! 6 }
//! ```
//!
//! Beyond what is shown here, a trampoline is also generated so the original
//! function can be called regardless whether the function is hooked or not.
//!
//! ## Caveats
//!
//! - Threads are not suspended whilst a target is being patched. Enabling or
//!   disabling a detour whilst another thread executes the target's prolog is
//!   undefined behavior.
//! - Multiple detours of the same target must be disabled in the reverse order
//!   they were enabled in; otherwise [`Error::TargetModified`] is returned.
//! - Under Rosetta 2 (x86-64 code on Apple silicon), modifying code whilst
//!   another thread executes the same memory page may intermittently raise
//!   `SIGBUS`. This is a limitation of the translator; native Intel Macs are
//!   not affected.
//!
//! ## Features
//!
//! - **std** (default): Uses the standard library for locking, and converts
//!   [`MemoryError`] into [`std::io::Error`].
//! - **no_std**: Supports `#![no_std]` environments with a global allocator,
//!   using spin locks. Disable the default features to use it:
//!
//!   ```toml
//!   detour = { version = "0.9", default-features = false, features = ["no_std"] }
//!   ```
//!
//!   On x86, `iced-x86` requires `std` and `no_std` to be mutually exclusive,
//!   so `no_std` cannot be used alongside another crate enabling `iced-x86/std`.

#![no_std]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

#[cfg(not(any(feature = "std", feature = "no_std")))]
compile_error!("either the `std` (default) or `no_std` feature of `detour` must be enabled");

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!(
  "detour only supports x86, x86-64 and AArch64 targets; inline detours are not possible on \
   architectures without addressable, writable code (e.g. WebAssembly)"
);

#[macro_use]
mod macros;

supported! {
  pub use detours::*;
  pub use error::{Error, MemoryError, Result};
  pub use traits::{Function, HookableWith};

  mod arch;
  mod detour;
  mod detours;
  mod error;
  mod memory;
  mod sync;
  mod traits;

  #[doc(hidden)]
  pub mod __private {
    pub use alloc::sync::Arc;
  }
}

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn detours_share_target() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) + y + std::hint::black_box(0) * line!() as i32
    }

    extern "C" fn sub(x: i32, y: i32) -> i32 {
      x - y
    }

    extern "C" fn div(x: i32, y: i32) -> i32 {
      x / y
    }

    // SAFETY: The functions share the same signature.
    let hook1 = unsafe { GenericDetour::<extern "C" fn(i32, i32) -> i32>::new(add, sub)? };
    // SAFETY: No other thread is executing `add`.
    unsafe { hook1.enable()? };
    assert_eq!(add(5, 5), 0);

    // SAFETY: The functions share the same signature.
    let hook2 = unsafe { GenericDetour::<extern "C" fn(i32, i32) -> i32>::new(add, div)? };
    // SAFETY: No other thread is executing `add`.
    unsafe { hook2.enable()? };

    // This will call the previous hook's detour
    assert_eq!(hook2.call(5, 5), 0);
    assert_eq!(add(10, 5), 2);

    // The first hook cannot be disabled before the second
    // SAFETY: No other thread is executing `add`.
    let result = unsafe { hook1.disable() };
    assert!(matches!(result, Err(Error::TargetModified)));
    // SAFETY: No other thread is executing `add`.
    unsafe {
      hook2.disable()?;
      hook1.disable()?;
    }
    assert_eq!(add(10, 5), 15);
    Ok(())
  }

  #[test]
  fn same_detour_and_target() {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      std::hint::black_box(x) + y + std::hint::black_box(0) * line!() as i32
    }

    // SAFETY: The detour is never enabled.
    let error = unsafe { RawDetour::new(add as *const (), add as *const ()) }.unwrap_err();
    assert!(matches!(error, Error::SameAddress));
  }

  #[test]
  fn non_executable_target() {
    let data = [0x90u8; 16];
    extern "C" fn detour() {}

    // SAFETY: The detour is never enabled.
    let error = unsafe { RawDetour::new(data.as_ptr().cast(), detour as *const ()) }.unwrap_err();
    assert!(matches!(error, Error::NotExecutable));
  }
}
