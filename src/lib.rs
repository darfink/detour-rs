//! Cross-platform function detouring (inline hooking) at runtime.
//!
//! A detour redirects a function (the *target*) to another function or
//! closure (the *detour*). The first instructions of the target are replaced
//! with a jump to the detour, and copied to a *trampoline*, through which the
//! original function remains callable.
//!
//! # Detours
//!
//! - [`static_detour!`]: defines a static, type-safe detour, which accepts a
//!   closure as its detour. Any signature is supported, including references.
//!   See [`example::Example`](struct@example::Example) for its methods.
//! - [`TypedDetour`]: a type-safe detour created at runtime, whose detour is a
//!   function with the same signature as the target. Signatures with
//!   references are named with [`signature!`] first.
//! - [`RawDetour`]: an untyped detour of raw pointers, e.g. for functions whose
//!   signature is only known at runtime.
//!
//! ```rust
//! use detour::static_detour;
//!
//! static_detour! {
//!   static Length: fn(&str) -> usize;
//! }
//!
//! #[inline(never)]
//! fn length(text: &str) -> usize {
//!   text.len()
//! }
//!
//! # fn main() -> detour::Result<()> {
//! // SAFETY: No other thread is executing `length`.
//! unsafe { Length.initialize(length, |text| Length.call(text) * 2)?.enable()? };
//!
//! assert_eq!(length("abc"), 6);
//! assert_eq!(Length.call("abc"), 3);
//! # Ok(())
//! # }
//! ```
//!
//! # Thread safety
//!
//! `enable` and `disable` do not suspend other threads. A thread executing
//! the target's first instructions whilst they are replaced may resume in the
//! middle of the new jump, so doing so is undefined behavior.
//!
//! A [`Transaction`] enables and disables several detours at once, applied
//! completely or not at all, whilst the chosen [`Threads`] are suspended. A
//! suspended thread stopped within the replaced instructions has its
//! instruction pointer moved to the same instruction in the trampoline
//! (known as *EIP relocation*). Suspending threads is supported on Windows,
//! Apple platforms, Linux and Android.
//!
//! See [`Transaction::commit`] for the limitations. On Linux and Android,
//! threads are suspended using a real-time signal (see
//! `linux::set_suspend_signal`).
//!
//! # How it works
//!
//! To illustrate a detour on x86:
//!
//! ```c
//! int return_five() {
//!     return 5;
//! 00400020 [b8 05 00 00 00] mov eax, 5
//! 00400025 [c3]             ret
//! }
//!
//! int detour_function() {
//!     return 10;
//! 00400040 [b8 0a 00 00 00] mov eax, 10
//! 00400045 [c3]             ret
//! }
//! ```
//!
//! The target's first instructions are disassembled, and relocated to a
//! trampoline allocated near the target, followed by a jump back to the rest
//! of the function. They are then replaced with a jump to the detour:
//!
//! ```c
//! int return_five() {
//!     return detour_function();
//! 00400020 [e9 1b 00 00 00] jmp 00400040 <detour_function>
//! 00400025 [c3]             ret
//! }
//! ```
//!
//! Relocation handles relative branches (including branches within the
//! replaced instructions), RIP-relative operands (x86-64) and all PC-relative
//! instructions (AArch64). If the detour is out of reach of a relative jump
//! (beyond ±2 GiB on x86-64, or ±128 MiB on AArch64), the jump leads to a
//! *relay*, an absolute jump allocated near the target. Functions too small
//! for the jump are supported if they are followed by padding, or preceded by
//! a hot-patching area. On AArch64, BTI and PAC landing pads are preserved.
//!
//! # Platforms
//!
//! - Architectures: `x86`, `x86-64` & `AArch64`.
//! - Operating systems: Windows, Linux, Android & macOS. Other Unix-like
//!   systems (e.g. FreeBSD) are expected to work, without thread suspension.
//!
//! On macOS, code signing prevents making `__TEXT` writable, so code is
//! patched by remapping the affected pages. Executable memory is allocated
//! with `MAP_JIT` on Apple silicon. Systems enforcing W^X are supported, as
//! long as code pages may be re-protected.
//!
//! # Caveats
//!
//! - Only calls that reach the target are detoured; inlined calls are not.
//!   Mark your own targets `#[inline(never)]`.
//! - A dropped detour is disabled without suspending threads, and its
//!   trampoline is released immediately. Disable it with a [`Transaction`]
//!   first if other threads may be executing the target or the trampoline.
//! - Multiple detours of the same target must be disabled in the reverse order
//!   they were enabled in; otherwise [`Error::TargetModified`] is returned.
//! - Under Rosetta 2 (x86-64 code on Apple silicon), modifying code whilst
//!   another thread executes the same memory page may intermittently raise
//!   `SIGBUS`. This is a limitation of the translator.
//!
//! # Features
//!
//! - **std** (default): Uses the standard library for locking, and allows
//!   converting [`OsError`] into `std::io::Error`, and a `JoinHandle` into a
//!   [`Thread`].
//!
//! Without `std`, `#![no_std]` environments with a global allocator are
//! supported, using spin locks. On x86, the `no_std` feature must be enabled
//! instead, since `iced-x86` requires either its `std` or its `no_std`
//! feature:
//!
//! ```toml
//! detour = { version = "0.9", default-features = false, features = ["no_std"] }
//! ```
//!
//! `iced-x86` does not allow both, so on x86 `no_std` cannot be combined
//! with another crate enabling `iced-x86/std`. On AArch64, the `no_std`
//! feature has no effect.

#![no_std]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

#[cfg(all(
  any(target_arch = "x86", target_arch = "x86_64"),
  not(any(feature = "std", feature = "no_std"))
))]
compile_error!(
  "on x86, either the `std` (default) or `no_std` feature of `detour` must be enabled, since \
   `iced-x86` requires one of them"
);

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!(
  "detour only supports x86, x86-64 and AArch64 targets; inline detours are not possible on \
   architectures without addressable, writable code (e.g. WebAssembly)"
);

#[macro_use]
mod macros;

supported! {
  pub use detours::*;
  pub use error::{Error, OsError, Result};
  pub use thread::{Thread, Threads};
  /// Functionality specific to Linux and Android.
  #[cfg(any(target_os = "linux", target_os = "android"))]
  pub mod linux {
    pub use crate::thread::set_suspend_signal;
  }
  pub use traits::{Function, HookableWith};
  pub use transaction::{Detour, Transaction};

  /// Implementation details of the macros; not part of the public API.
  #[doc(hidden)]
  pub mod __private {
    pub use crate::detours::statik::{StaticDetour, StaticHandle};
    pub use crate::traits::private::Sealed;
    pub use alloc::boxed::Box;
  }

  /// Example types generated by the macros (only part of the documentation).
  #[cfg(doc)]
  pub mod example {
    crate::static_detour! {
      /// An example of a static detour, as generated by:
      ///
      /// ```
      /// # use detour::static_detour;
      /// static_detour! {
      ///   pub static Example: fn(&str) -> usize;
      /// }
      /// ```
      pub static Example: fn(&str) -> usize;
    }

    crate::signature! {
      /// An example of a signature, as generated by:
      ///
      /// ```
      /// # use detour::signature;
      /// signature! {
      ///   pub struct Signature(pub fn(&str) -> usize);
      /// }
      /// ```
      pub struct Signature(pub fn(&str) -> usize);
    }
  }

  mod arch;
  mod detours;
  mod error;
  mod hook;
  mod memory;
  mod sync;
  mod thread;
  mod traits;
  mod transaction;
}

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// Code that must not compile, since it would be unsound.
///
/// A detour closure cannot require its arguments to outlive the call:
///
/// ```compile_fail,E0308
/// detour::static_detour! {
///   static Keep: fn(&str);
/// }
/// Keep.set_detour(|_: &'static str| {});
/// ```
///
/// Nor return a reference of the wrong lifetime:
///
/// ```compile_fail
/// detour::static_detour! {
///   static Pick: for<'a> fn(&'a str, &str) -> &'a str;
/// }
/// Pick.set_detour(|_, other| other);
/// ```
///
/// The original function of a typed detour cannot outlive it:
///
/// ```compile_fail,E0597
/// detour::signature! {
///   struct Length(fn(&str) -> usize);
/// }
/// fn length(text: &str) -> usize {
///   text.len()
/// }
/// let original = {
///   let hook = unsafe { detour::TypedDetour::new(Length(length), Length(|_| 0)) }.unwrap();
///   hook.original()
/// };
/// ```
///
/// ```compile_fail,E0597
/// fn square(value: i32) -> i32 {
///   value * value
/// }
/// let original = {
///   let hook =
///     unsafe { detour::TypedDetour::<fn(i32) -> i32>::new(square, |x| x) }.unwrap();
///   hook.original()
/// };
/// ```
///
/// Nor can the trampoline of a static detour's state that is not `'static`:
///
/// ```compile_fail,E0597
/// type Closure = dyn Fn() + Send + Sync;
/// fn ffi() {}
/// let trampoline = {
///   let state = unsafe { detour::__private::StaticDetour::<fn(), Closure>::__new(ffi) };
///   state.__trampoline()
/// };
/// ```
#[cfg(doctest)]
struct CompileFailDoctests;

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
    let hook1 = unsafe { TypedDetour::<extern "C" fn(i32, i32) -> i32>::new(add, sub)? };
    // SAFETY: No other thread is executing `add`.
    unsafe { hook1.enable()? };
    assert_eq!(add(5, 5), 0);

    // SAFETY: The functions share the same signature.
    let hook2 = unsafe { TypedDetour::<extern "C" fn(i32, i32) -> i32>::new(add, div)? };
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
