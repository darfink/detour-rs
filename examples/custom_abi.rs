//! Detours a function with a non-standard calling convention, using an
//! `extern "custom"` shim (Rust 1.100+) that adapts it to a standard one.
//!
//! Some functions pass their arguments in registers that no standard calling
//! convention uses: hand-written assembly, compiler intrinsics, and code
//! compiled with whole-program optimizations (e.g. `__usercall` functions, as
//! IDA calls them). Such a function cannot be described by a Rust function
//! pointer type, so it is detoured with [`RawDetour`] instead, and its detour
//! is a naked shim that moves the arguments where a Rust function expects
//! them.
//!
//! Declaring the shim (and the target) as `unsafe extern "custom" fn()`,
//! instead of pretending they are `extern "C"`, has the compiler reject any
//! direct call of them from Rust: they can only be called from assembly,
//! which states the calling convention explicitly.
//!
//! ```sh
//! $ cargo run --example custom_abi --features custom-abi
//! ```
//!
//! Supported on x86-64 & AArch64.

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
  use detour::RawDetour;
  use std::sync::atomic::Ordering;

  let scale = abi::scale as unsafe extern "custom" fn();

  // SAFETY: Both functions use the custom calling convention described in
  // `abi`.
  assert_eq!(unsafe { abi::call(scale, 6, 7) }, 42);

  // SAFETY: The shim implements the same calling convention as `scale`.
  let detour = unsafe { RawDetour::new(scale as *const (), abi::shim as *const ())? };
  abi::TRAMPOLINE.store(detour.trampoline().cast_mut(), Ordering::Release);

  // SAFETY: No other threads are executing `scale`.
  unsafe { detour.enable()? };
  // SAFETY: See above; calls now reach `scale_detour` through the shim.
  let detoured = unsafe { abi::call(scale, 6, 7) };
  println!("scale(6, 7) = {detoured} (detoured)");
  assert_eq!(detoured, 43);

  // SAFETY: No other threads are executing `scale`.
  unsafe { detour.disable()? };
  // SAFETY: See above.
  let original = unsafe { abi::call(scale, 6, 7) };
  println!("scale(6, 7) = {original} (original)");
  assert_eq!(original, 42);
  Ok(())
}

/// Called (by the shim) instead of `scale`, with a standard calling
/// convention.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
extern "C" fn scale_detour(value: u64, factor: u64) -> u64 {
  use std::sync::atomic::Ordering;

  // SAFETY: The trampoline is stored before the detour is enabled.
  let original: unsafe extern "custom" fn() =
    unsafe { core::mem::transmute(abi::TRAMPOLINE.load(Ordering::Acquire)) };
  // SAFETY: The trampoline invokes the original `scale`, so it shares its
  // calling convention, and it is valid whilst the detour exists (which
  // outlives every call of `scale` in `main`).
  unsafe { abi::call(original, value, factor) + 1 }
}

/// The custom calling convention of `scale`: the arguments are passed in
/// `rax` & `r12`, and the result is returned in `rax`. Otherwise, registers
/// are preserved as with `sysv64` (`r12` is callee-saved).
#[cfg(target_arch = "x86_64")]
mod abi {
  use core::arch::{asm, naked_asm};
  use std::sync::atomic::AtomicPtr;

  /// The trampoline of the detour, invoking the original `scale`.
  pub static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

  /// Returns `value * factor`; the function to detour.
  ///
  /// # Safety
  ///
  /// Must be called with the custom calling convention (see [`call`]).
  #[unsafe(naked)]
  pub unsafe extern "custom" fn scale() {
    naked_asm!(
      "push rbp",
      "mov rbp, rsp",
      "imul rax, r12",
      "pop rbp",
      "ret"
    );
  }

  /// Adapts the custom calling convention to `sysv64` (on Windows as well),
  /// and calls `scale_detour`.
  ///
  /// # Safety
  ///
  /// Must be called with the custom calling convention (see [`call`]).
  #[unsafe(naked)]
  pub unsafe extern "custom" fn shim() {
    naked_asm!(
      "push rbp",
      "mov rbp, rsp",
      // Do not trust the caller to align the stack
      "and rsp, -16",
      "mov rdi, rax",
      "mov rsi, r12",
      "call {detour}",
      // The result is already in `rax`
      "mov rsp, rbp",
      "pop rbp",
      "ret",
      detour = sym scale_detour,
    );
  }

  /// The detour, with the `sysv64` calling convention expected by the shim.
  extern "sysv64" fn scale_detour(value: u64, factor: u64) -> u64 {
    super::scale_detour(value, factor)
  }

  /// Calls a function with the custom calling convention.
  ///
  /// # Safety
  ///
  /// `function` must use the custom calling convention.
  pub unsafe fn call(function: unsafe extern "custom" fn(), value: u64, factor: u64) -> u64 {
    let result;
    // SAFETY: Forwarded from the caller; registers other than `rax` are
    // preserved as with `sysv64`.
    unsafe {
      asm!(
        "call {function}",
        function = in(reg) function,
        inout("rax") value => result,
        in("r12") factor,
        clobber_abi("sysv64"),
      );
    }
    result
  }
}

/// The custom calling convention of `scale`: the arguments are passed in `x9`
/// & `x10`, and the result is returned in `x9`. Otherwise, registers are
/// preserved as with `C`.
#[cfg(target_arch = "aarch64")]
mod abi {
  use core::arch::{asm, naked_asm};
  use std::sync::atomic::AtomicPtr;

  /// The trampoline of the detour, invoking the original `scale`.
  pub static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

  /// Returns `value * factor`; the function to detour.
  ///
  /// # Safety
  ///
  /// Must be called with the custom calling convention (see [`call`]).
  #[unsafe(naked)]
  pub unsafe extern "custom" fn scale() {
    naked_asm!(
      "stp x29, x30, [sp, #-16]!",
      "mov x29, sp",
      "mul x9, x9, x10",
      "ldp x29, x30, [sp], #16",
      "ret",
    );
  }

  /// Adapts the custom calling convention to `C`, and calls `scale_detour`.
  ///
  /// # Safety
  ///
  /// Must be called with the custom calling convention (see [`call`]).
  #[unsafe(naked)]
  pub unsafe extern "custom" fn shim() {
    naked_asm!(
      "stp x29, x30, [sp, #-16]!",
      "mov x29, sp",
      "mov x0, x9",
      "mov x1, x10",
      "bl {detour}",
      "mov x9, x0",
      "ldp x29, x30, [sp], #16",
      "ret",
      detour = sym super::scale_detour,
    );
  }

  /// Calls a function with the custom calling convention.
  ///
  /// # Safety
  ///
  /// `function` must use the custom calling convention.
  pub unsafe fn call(function: unsafe extern "custom" fn(), value: u64, factor: u64) -> u64 {
    let result;
    // SAFETY: Forwarded from the caller; registers other than `x9` are
    // preserved as with `C`.
    unsafe {
      asm!(
        "blr {function}",
        function = in(reg) function,
        inout("x9") value => result,
        inout("x10") factor => _,
        clobber_abi("C"),
      );
    }
    result
  }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn main() {
  println!("the custom_abi example only supports x86-64 & AArch64");
}
