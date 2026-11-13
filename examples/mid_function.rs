//! Detours an instruction in the middle of a function, using an
//! `extern "custom"` shim (Rust 1.100+) that saves the registers, lets a Rust
//! handler inspect & modify them, restores them, and resumes the function.
//!
//! This is what [`detour::ContextDetour`] does, with a shim generated at runtime;
//! use it instead, unless the shim must be customized (e.g. which state is
//! saved). This example writes the shim by hand, in assembly.
//!
//! A detour can be placed at any instruction, not only at the entry of a
//! function: the instructions overwritten by the jump are relocated to the
//! trampoline, which continues with the rest of the function. No calling
//! convention applies there, since the code is jumped to with the registers
//! (and flags) in use by the function. The shim must therefore preserve all
//! of them, and is entered (and left) by a jump instead of a call. Declaring
//! it as `extern "custom"` makes the compiler reject any call of it from
//! Rust.
//!
//! The function detoured here sums an array; the detour is placed after each
//! element is loaded, and clamps it to 100. In a real program, the address of
//! the instruction is found with a disassembler (e.g. as an offset into a
//! module), and its bytes are verified before the detour is created, as done
//! here, in case the program changes.
//!
//! The instructions overwritten by the jump must not be the target of other
//! branches. On AArch64, X16 must not hold a value at the instruction either,
//! since it is used as a scratch register (by this shim, and by detours that
//! use relays or absolute jumps).
//!
//! ```sh
//! $ cargo run --example mid_function --features custom-abi
//! ```
//!
//! Supported on x86-64 & AArch64.

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
  use detour::RawDetour;
  use std::sync::atomic::Ordering;

  let values = [5, 250, 40, 1000];
  assert_eq!(arch::total(&values), 1295);

  let hook_point = arch::total_raw as *const () as usize + arch::HOOK_OFFSET;
  // SAFETY: The function is at least as long as the expected instructions.
  let code = unsafe { core::slice::from_raw_parts(hook_point as *const u8, arch::HOOK_CODE.len()) };
  assert_eq!(
    code,
    arch::HOOK_CODE,
    "unexpected instructions at the hook point"
  );

  // SAFETY: The shim preserves the state at the hook point (see `arch`), and
  // no other code branches into the overwritten instructions.
  let detour = unsafe { RawDetour::new(hook_point as *const (), arch::shim as *const ())? };
  // The shim resumes the function through the trampoline
  arch::TRAMPOLINE.store(detour.trampoline().cast_mut(), Ordering::Release);

  // SAFETY: No other threads are executing `total`.
  unsafe { detour.enable()? };
  let detoured = arch::total(&values);
  println!("total({values:?}) = {detoured} (detoured)");
  assert_eq!(detoured, 5 + 100 + 40 + 100);

  // SAFETY: No other threads are executing `total`.
  unsafe { detour.disable()? };
  let original = arch::total(&values);
  println!("total({values:?}) = {original} (original)");
  assert_eq!(original, 1295);
  Ok(())
}

/// Called (by the shim) at the hook point, with the registers of `total`.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
extern "C" fn on_element(registers: &mut arch::Registers) {
  let element = registers.element();
  *element = (*element).min(100);
}

#[cfg(target_arch = "x86_64")]
mod arch {
  use core::arch::naked_asm;
  use std::sync::atomic::AtomicPtr;

  /// The offset of the hook point, `add rax, rcx` (followed by `dec rsi`),
  /// within `total_raw`.
  pub const HOOK_OFFSET: usize = 14;
  pub const HOOK_CODE: &[u8] = &[0x48, 0x01, 0xC8, 0x48, 0xFF, 0xCE];

  /// The trampoline of the detour, resuming `total` (read by the shim).
  pub static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

  /// The general-purpose registers & flags at the hook point, in the order
  /// the shim saves them (in reverse) onto the stack.
  #[repr(C)]
  pub struct Registers {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub rflags: u64,
  }

  impl Registers {
    /// The array element just loaded, about to be added to the sum.
    pub fn element(&mut self) -> &mut u64 {
      &mut self.rcx
    }
  }

  /// Returns the sum of `values`.
  pub fn total(values: &[u64]) -> u64 {
    total_raw(values.as_ptr(), values.len())
  }

  /// Sums `len` values at `values` (which must be valid for reads); the
  /// function to detour.
  #[unsafe(naked)]
  pub extern "sysv64" fn total_raw(values: *const u64, len: usize) -> u64 {
    naked_asm!(
      "xor eax, eax",
      "test rsi, rsi",
      "jz 3f",
      "2:",
      "mov rcx, [rdi]",
      "add rdi, 8",
      // The hook point (`HOOK_OFFSET`)
      "add rax, rcx",
      "dec rsi",
      "jnz 2b",
      "3:",
      "ret",
    );
  }

  /// Saves the state, calls `on_element`, restores the state, and resumes the
  /// function through the trampoline.
  ///
  /// # Safety
  ///
  /// Must only be jumped to, from the hook point.
  #[unsafe(naked)]
  pub unsafe extern "custom" fn shim() {
    naked_asm!(
      // Skip the red zone of the function, which it may use (System V)
      "lea rsp, [rsp - 128]",
      "pushfq",
      "push rax",
      "push rcx",
      "push rdx",
      "push rbx",
      "push rbp",
      "push rsi",
      "push rdi",
      "push r8",
      "push r9",
      "push r10",
      "push r11",
      "push r12",
      "push r13",
      "push r14",
      "push r15",
      // Required upon calls; restored with the flags
      "cld",
      // Save the vector registers & MXCSR on an aligned stack, and keep the
      // address of the registers in a callee-saved register
      "mov rbx, rsp",
      "and rsp, -16",
      "sub rsp, 512",
      "fxsave64 [rsp]",
      "mov rdi, rbx",
      "call {handler}",
      "fxrstor64 [rsp]",
      "mov rsp, rbx",
      "pop r15",
      "pop r14",
      "pop r13",
      "pop r12",
      "pop r11",
      "pop r10",
      "pop r9",
      "pop r8",
      "pop rdi",
      "pop rsi",
      "pop rbp",
      "pop rbx",
      "pop rdx",
      "pop rcx",
      "pop rax",
      "popfq",
      // `LEA` preserves the flags
      "lea rsp, [rsp + 128]",
      "jmp qword ptr [rip + {trampoline}]",
      handler = sym on_element,
      trampoline = sym TRAMPOLINE,
    );
  }

  /// The handler, with the `sysv64` calling convention expected by the shim
  /// (on Windows as well).
  extern "sysv64" fn on_element(registers: &mut Registers) {
    super::on_element(registers);
  }
}

#[cfg(target_arch = "aarch64")]
mod arch {
  use core::arch::naked_asm;
  use std::sync::atomic::AtomicPtr;

  /// The offset of the hook point, `add x0, x0, x10`, within `total_raw`.
  pub const HOOK_OFFSET: usize = 16;
  pub const HOOK_CODE: &[u8] = &0x8B0A_0000_u32.to_le_bytes();

  /// The trampoline of the detour, resuming `total` (read by the shim).
  pub static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

  /// The general-purpose registers & flags at the hook point, in the order
  /// the shim saves them onto the stack.
  #[repr(C)]
  pub struct Registers {
    /// X0 to X30 (the link register).
    pub x: [u64; 31],
    pub nzcv: u64,
  }

  impl Registers {
    /// The array element just loaded, about to be added to the sum.
    pub fn element(&mut self) -> &mut u64 {
      &mut self.x[10]
    }
  }

  /// Returns the sum of `values`.
  pub fn total(values: &[u64]) -> u64 {
    total_raw(values.as_ptr(), values.len())
  }

  /// Sums `len` values at `values` (which must be valid for reads); the
  /// function to detour.
  #[unsafe(naked)]
  pub extern "C" fn total_raw(values: *const u64, len: usize) -> u64 {
    naked_asm!(
      "mov x9, x0",
      "mov x0, #0",
      "cbz x1, 3f",
      "2:",
      "ldr x10, [x9], #8",
      // The hook point (`HOOK_OFFSET`)
      "add x0, x0, x10",
      "subs x1, x1, #1",
      "b.ne 2b",
      "3:",
      "ret",
    );
  }

  /// Saves the state, calls `on_element`, restores the state, and resumes the
  /// function through the trampoline (clobbering X16).
  ///
  /// The frame holds the general-purpose registers & NZCV (256 bytes), the
  /// vector registers (512 bytes), and FPSR & FPCR (16 bytes).
  ///
  /// # Safety
  ///
  /// Must only be jumped to, from the hook point.
  #[unsafe(naked)]
  pub unsafe extern "custom" fn shim() {
    naked_asm!(
      // Skip the red zone of the function, which it may use (Apple), and
      // allocate the frame
      "sub sp, sp, #(128 + 784)",
      "stp x0, x1, [sp, #0]",
      "stp x2, x3, [sp, #16]",
      "stp x4, x5, [sp, #32]",
      "stp x6, x7, [sp, #48]",
      "stp x8, x9, [sp, #64]",
      "stp x10, x11, [sp, #80]",
      "stp x12, x13, [sp, #96]",
      "stp x14, x15, [sp, #112]",
      "stp x16, x17, [sp, #128]",
      "stp x18, x19, [sp, #144]",
      "stp x20, x21, [sp, #160]",
      "stp x22, x23, [sp, #176]",
      "stp x24, x25, [sp, #192]",
      "stp x26, x27, [sp, #208]",
      "stp x28, x29, [sp, #224]",
      "mrs x9, nzcv",
      "stp x30, x9, [sp, #240]",
      "stp q0, q1, [sp, #256]",
      "stp q2, q3, [sp, #288]",
      "stp q4, q5, [sp, #320]",
      "stp q6, q7, [sp, #352]",
      "stp q8, q9, [sp, #384]",
      "stp q10, q11, [sp, #416]",
      "stp q12, q13, [sp, #448]",
      "stp q14, q15, [sp, #480]",
      "stp q16, q17, [sp, #512]",
      "stp q18, q19, [sp, #544]",
      "stp q20, q21, [sp, #576]",
      "stp q22, q23, [sp, #608]",
      "stp q24, q25, [sp, #640]",
      "stp q26, q27, [sp, #672]",
      "stp q28, q29, [sp, #704]",
      "stp q30, q31, [sp, #736]",
      "mrs x9, fpsr",
      "mrs x10, fpcr",
      "str x9, [sp, #768]",
      "str x10, [sp, #776]",
      "mov x0, sp",
      "bl {handler}",
      "ldr x9, [sp, #768]",
      "ldr x10, [sp, #776]",
      "msr fpsr, x9",
      "msr fpcr, x10",
      "ldp q0, q1, [sp, #256]",
      "ldp q2, q3, [sp, #288]",
      "ldp q4, q5, [sp, #320]",
      "ldp q6, q7, [sp, #352]",
      "ldp q8, q9, [sp, #384]",
      "ldp q10, q11, [sp, #416]",
      "ldp q12, q13, [sp, #448]",
      "ldp q14, q15, [sp, #480]",
      "ldp q16, q17, [sp, #512]",
      "ldp q18, q19, [sp, #544]",
      "ldp q20, q21, [sp, #576]",
      "ldp q22, q23, [sp, #608]",
      "ldp q24, q25, [sp, #640]",
      "ldp q26, q27, [sp, #672]",
      "ldp q28, q29, [sp, #704]",
      "ldp q30, q31, [sp, #736]",
      "ldp x30, x9, [sp, #240]",
      "msr nzcv, x9",
      "ldp x0, x1, [sp, #0]",
      "ldp x2, x3, [sp, #16]",
      "ldp x4, x5, [sp, #32]",
      "ldp x6, x7, [sp, #48]",
      "ldp x8, x9, [sp, #64]",
      "ldp x10, x11, [sp, #80]",
      "ldp x12, x13, [sp, #96]",
      "ldp x14, x15, [sp, #112]",
      // X16 is overwritten below
      "ldr x17, [sp, #136]",
      "ldp x18, x19, [sp, #144]",
      "ldp x20, x21, [sp, #160]",
      "ldp x22, x23, [sp, #176]",
      "ldp x24, x25, [sp, #192]",
      "ldp x26, x27, [sp, #208]",
      "ldp x28, x29, [sp, #224]",
      "add sp, sp, #(128 + 784)",
      #[cfg(target_vendor = "apple")]
      "adrp x16, {trampoline}@PAGE",
      #[cfg(target_vendor = "apple")]
      "ldr x16, [x16, {trampoline}@PAGEOFF]",
      #[cfg(not(target_vendor = "apple"))]
      "adrp x16, {trampoline}",
      #[cfg(not(target_vendor = "apple"))]
      "ldr x16, [x16, :lo12:{trampoline}]",
      "br x16",
      handler = sym super::on_element,
      trampoline = sym TRAMPOLINE,
    );
  }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn main() {
  println!("the mid_function example only supports x86-64 & AArch64");
}
