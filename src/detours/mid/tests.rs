//! Tests of the shims, comparing the state after a hook point with and
//! without a detour.
//!
//! A probe loads all registers (general-purpose, flags & vector) from an
//! input, executes a hook point (a few `NOP`s), and stores them to an output,
//! along with the address of the hook point and the stack pointer there.
//!
//! The tests share the probe, so they are serialized by [`PROBE`].

use super::{Context, MidDetour};
use crate::error::Result;
use core::hint::black_box;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::format;
use std::string::String;
use std::sync::Mutex;
use std::vec::Vec;

/// Serializes the tests, which detour the same probe.
static PROBE: Mutex<()> = Mutex::new(());

#[cfg(target_arch = "x86_64")]
mod probe {
  use core::arch::naked_asm;

  /// The state loaded & stored by the probe.
  #[repr(C, align(16))]
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct State {
    /// The general-purpose registers, by number (RSP is not loaded).
    pub r: [u64; 16],
    pub flags: u64,
    _padding: u64,
    pub xmm: [u128; 16],
    /// The address of the hook point (output only).
    pub point: u64,
    /// The stack pointer at the hook point (output only).
    pub sp: u64,
    /// Sentinels written to the lowest & highest words of the red zone
    /// before the hook point, and read after it (output only).
    pub red_zone: [u64; 2],
  }

  impl State {
    pub fn new(seed: u64) -> Self {
      let pattern = |index: u64| (seed ^ index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
      State {
        r: core::array::from_fn(|index| pattern(index as u64)),
        // CF, PF, ZF & OF (and the reserved bit)
        flags: 0x2 | 0x1 | 0x4 | 0x40 | 0x800,
        _padding: 0,
        xmm: core::array::from_fn(|index| u128::from(pattern(100 + index as u64)) << 64 | u128::from(pattern(index as u64))),
        point: 0,
        sp: 0,
        red_zone: [0; 2],
      }
    }

    /// Clears the registers that are not compared.
    pub fn compared(mut self) -> Self {
      self.r[4] = 0;
      self.sp = 0;
      self
    }
  }

  /// Loads `input`, executes the hook point, and stores the state to `output`.
  #[unsafe(naked)]
  pub unsafe extern "sysv64" fn probe(input: *const State, output: *mut State) {
    naked_asm!(
      "push rbx",
      "push rbp",
      "push r12",
      "push r13",
      "push r14",
      "push r15",
      "push rsi",
      "lea rax, [rip + 2f]",
      "mov [rsi + 400], rax",
      "mov [rsi + 408], rsp",
      // Red zone sentinels (the word below the stack pointer is overwritten
      // when storing the state, so the next one is used)
      "mov rax, 0x5AFE5AFE5AFE5AFE",
      "mov [rsp - 16], rax",
      "mov [rsp - 128], rax",
      "movdqu xmm0, [rdi + 144]",
      "movdqu xmm1, [rdi + 160]",
      "movdqu xmm2, [rdi + 176]",
      "movdqu xmm3, [rdi + 192]",
      "movdqu xmm4, [rdi + 208]",
      "movdqu xmm5, [rdi + 224]",
      "movdqu xmm6, [rdi + 240]",
      "movdqu xmm7, [rdi + 256]",
      "movdqu xmm8, [rdi + 272]",
      "movdqu xmm9, [rdi + 288]",
      "movdqu xmm10, [rdi + 304]",
      "movdqu xmm11, [rdi + 320]",
      "movdqu xmm12, [rdi + 336]",
      "movdqu xmm13, [rdi + 352]",
      "movdqu xmm14, [rdi + 368]",
      "movdqu xmm15, [rdi + 384]",
      "push qword ptr [rdi + 128]",
      "popfq",
      "mov rax, [rdi]",
      "mov rcx, [rdi + 8]",
      "mov rdx, [rdi + 16]",
      "mov rbx, [rdi + 24]",
      "mov rbp, [rdi + 40]",
      "mov rsi, [rdi + 48]",
      "mov r8, [rdi + 64]",
      "mov r9, [rdi + 72]",
      "mov r10, [rdi + 80]",
      "mov r11, [rdi + 88]",
      "mov r12, [rdi + 96]",
      "mov r13, [rdi + 104]",
      "mov r14, [rdi + 112]",
      "mov r15, [rdi + 120]",
      "mov rdi, [rdi + 56]",
      // The hook point
      "2:",
      ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
      "push rax",
      "mov rax, [rsp + 8]",
      "mov [rax + 8], rcx",
      "mov rcx, [rsp - 8]",
      "mov [rax + 416], rcx",
      "mov rcx, [rsp - 120]",
      "mov [rax + 424], rcx",
      "mov [rax + 16], rdx",
      "mov [rax + 24], rbx",
      "mov [rax + 40], rbp",
      "mov [rax + 48], rsi",
      "mov [rax + 56], rdi",
      "mov [rax + 64], r8",
      "mov [rax + 72], r9",
      "mov [rax + 80], r10",
      "mov [rax + 88], r11",
      "mov [rax + 96], r12",
      "mov [rax + 104], r13",
      "mov [rax + 112], r14",
      "mov [rax + 120], r15",
      "pushfq",
      "pop qword ptr [rax + 128]",
      "pop qword ptr [rax]",
      "movdqu [rax + 144], xmm0",
      "movdqu [rax + 160], xmm1",
      "movdqu [rax + 176], xmm2",
      "movdqu [rax + 192], xmm3",
      "movdqu [rax + 208], xmm4",
      "movdqu [rax + 224], xmm5",
      "movdqu [rax + 240], xmm6",
      "movdqu [rax + 256], xmm7",
      "movdqu [rax + 272], xmm8",
      "movdqu [rax + 288], xmm9",
      "movdqu [rax + 304], xmm10",
      "movdqu [rax + 320], xmm11",
      "movdqu [rax + 336], xmm12",
      "movdqu [rax + 352], xmm13",
      "movdqu [rax + 368], xmm14",
      "movdqu [rax + 384], xmm15",
      "add rsp, 8",
      "pop r15",
      "pop r14",
      "pop r13",
      "pop r12",
      "pop rbp",
      "pop rbx",
      "ret",
    );
  }

  pub type Word = u64;
  /// The flag toggled in the context (CF).
  pub const FLAG: Word = 1;

  /// Returns the general-purpose registers of the context, by number, and
  /// its flags.
  pub fn registers(context: &mut super::Context) -> (&mut [Word], Option<&mut Word>) {
    // SAFETY: The context consists of the registers by number, followed by
    // the flags.
    let words = unsafe { &mut *(&raw mut *context).cast::<[Word; 17]>() };
    let (registers, flags) = words.split_at_mut(16);
    (registers, Some(&mut flags[0]))
  }

  /// Clobbers the vector registers.
  pub fn clobber_vectors() {
    // SAFETY: The clobbered registers are declared.
    unsafe {
      core::arch::asm!(
        "xorps xmm0, xmm0", "xorps xmm1, xmm1", "xorps xmm2, xmm2", "xorps xmm3, xmm3",
        "xorps xmm4, xmm4", "xorps xmm5, xmm5", "xorps xmm6, xmm6", "xorps xmm7, xmm7",
        "xorps xmm8, xmm8", "xorps xmm9, xmm9", "xorps xmm10, xmm10", "xorps xmm11, xmm11",
        "xorps xmm12, xmm12", "xorps xmm13, xmm13", "xorps xmm14, xmm14", "xorps xmm15, xmm15",
        out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
        out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
        out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
        out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
      );
    }
  }

  pub fn sp(context: &super::Context) -> u64 {
    context.rsp()
  }
}

#[cfg(target_arch = "aarch64")]
mod probe {
  use core::arch::naked_asm;

  /// The state loaded & stored by the probe.
  #[repr(C, align(16))]
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct State {
    /// X0 to X30 (X16, the scratch register, and X18, the platform register,
    /// are neither loaded nor compared).
    pub r: [u64; 31],
    pub flags: u64,
    pub q: [u128; 32],
    /// The address of the hook point (output only).
    pub point: u64,
    /// The stack pointer at the hook point (output only).
    pub sp: u64,
  }

  impl State {
    pub fn new(seed: u64) -> Self {
      let pattern = |index: u64| (seed ^ index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
      State {
        r: core::array::from_fn(|index| pattern(index as u64)),
        // N, C & V
        flags: 0xB000_0000,
        q: core::array::from_fn(|index| u128::from(pattern(100 + index as u64)) << 64 | u128::from(pattern(index as u64))),
        point: 0,
        sp: 0,
      }
    }

    /// Clears the registers that are not compared.
    pub fn compared(mut self) -> Self {
      self.r[16] = 0;
      self.r[18] = 0;
      self.sp = 0;
      self
    }
  }

  /// Loads `input`, executes the hook point, and stores the state to `output`.
  #[unsafe(naked)]
  pub unsafe extern "C" fn probe(input: *const State, output: *mut State) {
    naked_asm!(
      "stp x29, x30, [sp, #-16]!",
      "stp x19, x20, [sp, #-16]!",
      "stp x21, x22, [sp, #-16]!",
      "stp x23, x24, [sp, #-16]!",
      "stp x25, x26, [sp, #-16]!",
      "stp x27, x28, [sp, #-16]!",
      "stp d8, d9, [sp, #-16]!",
      "stp d10, d11, [sp, #-16]!",
      "stp d12, d13, [sp, #-16]!",
      "stp d14, d15, [sp, #-16]!",
      "stp x1, xzr, [sp, #-16]!",
      "adr x9, 2f",
      "str x9, [x1, #768]",
      "mov x9, sp",
      "str x9, [x1, #776]",
      "ldp q0, q1, [x0, #256]",
      "ldp q2, q3, [x0, #288]",
      "ldp q4, q5, [x0, #320]",
      "ldp q6, q7, [x0, #352]",
      "ldp q8, q9, [x0, #384]",
      "ldp q10, q11, [x0, #416]",
      "ldp q12, q13, [x0, #448]",
      "ldp q14, q15, [x0, #480]",
      "ldp q16, q17, [x0, #512]",
      "ldp q18, q19, [x0, #544]",
      "ldp q20, q21, [x0, #576]",
      "ldp q22, q23, [x0, #608]",
      "ldp q24, q25, [x0, #640]",
      "ldp q26, q27, [x0, #672]",
      "ldp q28, q29, [x0, #704]",
      "ldp q30, q31, [x0, #736]",
      "ldr x9, [x0, #248]",
      "msr nzcv, x9",
      "ldr x1, [x0, #8]",
      "ldp x2, x3, [x0, #16]",
      "ldp x4, x5, [x0, #32]",
      "ldp x6, x7, [x0, #48]",
      "ldp x8, x9, [x0, #64]",
      "ldp x10, x11, [x0, #80]",
      "ldp x12, x13, [x0, #96]",
      "ldp x14, x15, [x0, #112]",
      "ldr x17, [x0, #136]",
      "ldp x19, x20, [x0, #152]",
      "ldp x21, x22, [x0, #168]",
      "ldp x23, x24, [x0, #184]",
      "ldp x25, x26, [x0, #200]",
      "ldp x27, x28, [x0, #216]",
      "ldp x29, x30, [x0, #232]",
      "ldr x0, [x0]",
      // The hook point
      "2:",
      "nop",
      "nop",
      "nop",
      "nop",
      // X16 is the scratch register, which holds no value
      "ldr x16, [sp]",
      "str x0, [x16]",
      "str x1, [x16, #8]",
      "stp x2, x3, [x16, #16]",
      "stp x4, x5, [x16, #32]",
      "stp x6, x7, [x16, #48]",
      "stp x8, x9, [x16, #64]",
      "stp x10, x11, [x16, #80]",
      "stp x12, x13, [x16, #96]",
      "stp x14, x15, [x16, #112]",
      "str x17, [x16, #136]",
      "stp x19, x20, [x16, #152]",
      "stp x21, x22, [x16, #168]",
      "stp x23, x24, [x16, #184]",
      "stp x25, x26, [x16, #200]",
      "stp x27, x28, [x16, #216]",
      "stp x29, x30, [x16, #232]",
      "mrs x9, nzcv",
      "str x9, [x16, #248]",
      "stp q0, q1, [x16, #256]",
      "stp q2, q3, [x16, #288]",
      "stp q4, q5, [x16, #320]",
      "stp q6, q7, [x16, #352]",
      "stp q8, q9, [x16, #384]",
      "stp q10, q11, [x16, #416]",
      "stp q12, q13, [x16, #448]",
      "stp q14, q15, [x16, #480]",
      "stp q16, q17, [x16, #512]",
      "stp q18, q19, [x16, #544]",
      "stp q20, q21, [x16, #576]",
      "stp q22, q23, [x16, #608]",
      "stp q24, q25, [x16, #640]",
      "stp q26, q27, [x16, #672]",
      "stp q28, q29, [x16, #704]",
      "stp q30, q31, [x16, #736]",
      "add sp, sp, #16",
      "ldp d14, d15, [sp], #16",
      "ldp d12, d13, [sp], #16",
      "ldp d10, d11, [sp], #16",
      "ldp d8, d9, [sp], #16",
      "ldp x27, x28, [sp], #16",
      "ldp x25, x26, [sp], #16",
      "ldp x23, x24, [sp], #16",
      "ldp x21, x22, [sp], #16",
      "ldp x19, x20, [sp], #16",
      "ldp x29, x30, [sp], #16",
      "ret",
    );
  }

  pub type Word = u64;
  /// The flag toggled in the context (C).
  pub const FLAG: Word = 1 << 29;

  /// Returns the general-purpose registers of the context, by number, and
  /// its flags.
  pub fn registers(context: &mut super::Context) -> (&mut [Word], Option<&mut Word>) {
    (&mut context.x, Some(&mut context.nzcv))
  }

  /// Clobbers the vector registers.
  pub fn clobber_vectors() {
    // SAFETY: The clobbered registers are declared.
    unsafe {
      core::arch::asm!(
        "movi v0.16b, #0", "movi v1.16b, #0", "movi v2.16b, #0", "movi v3.16b, #0",
        "movi v4.16b, #0", "movi v5.16b, #0", "movi v6.16b, #0", "movi v7.16b, #0",
        "movi v8.16b, #0", "movi v9.16b, #0", "movi v10.16b, #0", "movi v11.16b, #0",
        "movi v12.16b, #0", "movi v13.16b, #0", "movi v14.16b, #0", "movi v15.16b, #0",
        "movi v16.16b, #0", "movi v17.16b, #0", "movi v18.16b, #0", "movi v19.16b, #0",
        "movi v20.16b, #0", "movi v21.16b, #0", "movi v22.16b, #0", "movi v23.16b, #0",
        "movi v24.16b, #0", "movi v25.16b, #0", "movi v26.16b, #0", "movi v27.16b, #0",
        "movi v28.16b, #0", "movi v29.16b, #0", "movi v30.16b, #0", "movi v31.16b, #0",
        out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _,
        out("v8") _, out("v9") _, out("v10") _, out("v11") _, out("v12") _, out("v13") _, out("v14") _, out("v15") _,
        out("v16") _, out("v17") _, out("v18") _, out("v19") _, out("v20") _, out("v21") _, out("v22") _, out("v23") _,
        out("v24") _, out("v25") _, out("v26") _, out("v27") _, out("v28") _, out("v29") _, out("v30") _, out("v31") _,
      );
    }
  }

  pub fn sp(context: &super::Context) -> u64 {
    context.sp()
  }
}

#[cfg(target_arch = "x86")]
mod probe {
  use core::arch::naked_asm;

  /// The state loaded & stored by the probe.
  #[repr(C, align(16))]
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct State {
    /// The general-purpose registers, by number (ESP is not loaded).
    pub r: [u32; 8],
    pub flags: u32,
    _padding: [u32; 3],
    pub xmm: [u128; 8],
    /// The address of the hook point (output only).
    pub point: u32,
    /// The stack pointer at the hook point (output only).
    pub sp: u32,
  }

  impl State {
    pub fn new(seed: u64) -> Self {
      let pattern = |index: u64| (seed ^ index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
      State {
        r: core::array::from_fn(|index| pattern(index as u64) as u32),
        // CF, PF, ZF & OF (and the reserved bit)
        flags: 0x2 | 0x1 | 0x4 | 0x40 | 0x800,
        _padding: [0; 3],
        xmm: core::array::from_fn(|index| u128::from(pattern(100 + index as u64)) << 64 | u128::from(pattern(index as u64))),
        point: 0,
        sp: 0,
      }
    }

    /// Clears the registers that are not compared.
    pub fn compared(mut self) -> Self {
      self.r[4] = 0;
      self.sp = 0;
      self
    }
  }

  /// Loads `input`, executes the hook point, and stores the state to `output`.
  #[unsafe(naked)]
  pub unsafe extern "C" fn probe(input: *const State, output: *mut State) {
    naked_asm!(
      "push ebx",
      "push ebp",
      "push esi",
      "push edi",
      "mov edi, [esp + 20]",
      "mov esi, [esp + 24]",
      "push esi",
      "call 3f",
      "3:",
      "pop eax",
      // add eax, 2f - 3b (a label difference is parsed as a memory operand)
      ".byte 0x05",
      ".long 2f - 3b",
      "mov [esi + 176], eax",
      "mov [esi + 180], esp",
      "movdqu xmm0, [edi + 48]",
      "movdqu xmm1, [edi + 64]",
      "movdqu xmm2, [edi + 80]",
      "movdqu xmm3, [edi + 96]",
      "movdqu xmm4, [edi + 112]",
      "movdqu xmm5, [edi + 128]",
      "movdqu xmm6, [edi + 144]",
      "movdqu xmm7, [edi + 160]",
      "push dword ptr [edi + 32]",
      "popfd",
      "mov eax, [edi]",
      "mov ecx, [edi + 4]",
      "mov edx, [edi + 8]",
      "mov ebx, [edi + 12]",
      "mov ebp, [edi + 20]",
      "mov esi, [edi + 24]",
      "mov edi, [edi + 28]",
      // The hook point
      "2:",
      ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
      "push eax",
      "mov eax, [esp + 4]",
      "mov [eax + 4], ecx",
      "mov [eax + 8], edx",
      "mov [eax + 12], ebx",
      "mov [eax + 20], ebp",
      "mov [eax + 24], esi",
      "mov [eax + 28], edi",
      "pushfd",
      "pop dword ptr [eax + 32]",
      "pop dword ptr [eax]",
      "movdqu [eax + 48], xmm0",
      "movdqu [eax + 64], xmm1",
      "movdqu [eax + 80], xmm2",
      "movdqu [eax + 96], xmm3",
      "movdqu [eax + 112], xmm4",
      "movdqu [eax + 128], xmm5",
      "movdqu [eax + 144], xmm6",
      "movdqu [eax + 160], xmm7",
      "add esp, 4",
      "pop edi",
      "pop esi",
      "pop ebp",
      "pop ebx",
      "ret",
    );
  }

  pub type Word = u32;
  /// The flag toggled in the context (CF).
  pub const FLAG: Word = 1;

  /// Returns the general-purpose registers of the context, by number, and
  /// its flags.
  pub fn registers(context: &mut super::Context) -> (&mut [Word], Option<&mut Word>) {
    // SAFETY: The context consists of the registers by number, followed by
    // the flags.
    let words = unsafe { &mut *(&raw mut *context).cast::<[Word; 9]>() };
    let (registers, flags) = words.split_at_mut(8);
    (registers, Some(&mut flags[0]))
  }

  /// Clobbers the vector registers.
  pub fn clobber_vectors() {
    // SAFETY: The clobbered registers are declared.
    unsafe {
      core::arch::asm!(
        "xorps xmm0, xmm0", "xorps xmm1, xmm1", "xorps xmm2, xmm2", "xorps xmm3, xmm3",
        "xorps xmm4, xmm4", "xorps xmm5, xmm5", "xorps xmm6, xmm6", "xorps xmm7, xmm7",
        out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
        out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
      );
    }
  }

  pub fn sp(context: &super::Context) -> u32 {
    context.esp()
  }
}

#[cfg(target_arch = "riscv64")]
mod probe {
  use core::arch::naked_asm;

  /// The state loaded & stored by the probe.
  #[repr(C, align(16))]
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct State {
    /// X0 to X31, by number (X0, SP, GP & TP, and T1, the scratch register,
    /// are neither loaded nor compared).
    pub r: [u64; 32],
    /// FCSR
    pub flags: u64,
    /// F0 to F31
    pub f: [u64; 32],
    /// The address of the hook point (output only).
    pub point: u64,
    /// The stack pointer at the hook point (output only).
    pub sp: u64,
  }

  impl State {
    pub fn new(seed: u64) -> Self {
      let pattern = |index: u64| (seed ^ index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
      State {
        r: core::array::from_fn(|index| pattern(index as u64)),
        // Rounding towards zero, and the NV, OF & NX flags
        flags: 1 << 5 | 0b10101,
        f: core::array::from_fn(|index| pattern(100 + index as u64)),
        point: 0,
        sp: 0,
      }
    }

    /// Clears the registers that are not compared.
    pub fn compared(mut self) -> Self {
      for register in [0, 2, 3, 4, 6] {
        self.r[register] = 0;
      }
      self.sp = 0;
      self
    }
  }

  /// Loads `input`, executes the hook point, and stores the state to `output`.
  #[unsafe(naked)]
  pub unsafe extern "C" fn probe(input: *const State, output: *mut State) {
    naked_asm!(
      "addi sp, sp, -208",
      "sd ra, 0(sp)",
      "sd s0, 8(sp)",
      "sd s1, 16(sp)",
      "sd s2, 24(sp)",
      "sd s3, 32(sp)",
      "sd s4, 40(sp)",
      "sd s5, 48(sp)",
      "sd s6, 56(sp)",
      "sd s7, 64(sp)",
      "sd s8, 72(sp)",
      "sd s9, 80(sp)",
      "sd s10, 88(sp)",
      "sd s11, 96(sp)",
      "fsd fs0, 104(sp)",
      "fsd fs1, 112(sp)",
      "fsd fs2, 120(sp)",
      "fsd fs3, 128(sp)",
      "fsd fs4, 136(sp)",
      "fsd fs5, 144(sp)",
      "fsd fs6, 152(sp)",
      "fsd fs7, 160(sp)",
      "fsd fs8, 168(sp)",
      "fsd fs9, 176(sp)",
      "fsd fs10, 184(sp)",
      "fsd fs11, 192(sp)",
      "sd a1, 200(sp)",
      "lla t0, 2f",
      "sd t0, 520(a1)",
      "sd sp, 528(a1)",
      "ld t0, 256(a0)",
      "fscsr t0",
      "fld f0, 264(a0)",
      "fld f1, 272(a0)",
      "fld f2, 280(a0)",
      "fld f3, 288(a0)",
      "fld f4, 296(a0)",
      "fld f5, 304(a0)",
      "fld f6, 312(a0)",
      "fld f7, 320(a0)",
      "fld f8, 328(a0)",
      "fld f9, 336(a0)",
      "fld f10, 344(a0)",
      "fld f11, 352(a0)",
      "fld f12, 360(a0)",
      "fld f13, 368(a0)",
      "fld f14, 376(a0)",
      "fld f15, 384(a0)",
      "fld f16, 392(a0)",
      "fld f17, 400(a0)",
      "fld f18, 408(a0)",
      "fld f19, 416(a0)",
      "fld f20, 424(a0)",
      "fld f21, 432(a0)",
      "fld f22, 440(a0)",
      "fld f23, 448(a0)",
      "fld f24, 456(a0)",
      "fld f25, 464(a0)",
      "fld f26, 472(a0)",
      "fld f27, 480(a0)",
      "fld f28, 488(a0)",
      "fld f29, 496(a0)",
      "fld f30, 504(a0)",
      "fld f31, 512(a0)",
      "ld x1, 8(a0)",
      "ld x5, 40(a0)",
      "ld x7, 56(a0)",
      "ld x8, 64(a0)",
      "ld x9, 72(a0)",
      "ld x11, 88(a0)",
      "ld x12, 96(a0)",
      "ld x13, 104(a0)",
      "ld x14, 112(a0)",
      "ld x15, 120(a0)",
      "ld x16, 128(a0)",
      "ld x17, 136(a0)",
      "ld x18, 144(a0)",
      "ld x19, 152(a0)",
      "ld x20, 160(a0)",
      "ld x21, 168(a0)",
      "ld x22, 176(a0)",
      "ld x23, 184(a0)",
      "ld x24, 192(a0)",
      "ld x25, 200(a0)",
      "ld x26, 208(a0)",
      "ld x27, 216(a0)",
      "ld x28, 224(a0)",
      "ld x29, 232(a0)",
      "ld x30, 240(a0)",
      "ld x31, 248(a0)",
      "ld x10, 80(a0)",
      // The hook point, without compressed instructions
      ".option push",
      ".option norvc",
      "2:",
      "nop",
      "nop",
      "nop",
      "nop",
      ".option pop",
      // T1 is the scratch register, which holds no value
      "ld t1, 200(sp)",
      "sd x1, 8(t1)",
      "sd x5, 40(t1)",
      "sd x7, 56(t1)",
      "sd x8, 64(t1)",
      "sd x9, 72(t1)",
      "sd x10, 80(t1)",
      "sd x11, 88(t1)",
      "sd x12, 96(t1)",
      "sd x13, 104(t1)",
      "sd x14, 112(t1)",
      "sd x15, 120(t1)",
      "sd x16, 128(t1)",
      "sd x17, 136(t1)",
      "sd x18, 144(t1)",
      "sd x19, 152(t1)",
      "sd x20, 160(t1)",
      "sd x21, 168(t1)",
      "sd x22, 176(t1)",
      "sd x23, 184(t1)",
      "sd x24, 192(t1)",
      "sd x25, 200(t1)",
      "sd x26, 208(t1)",
      "sd x27, 216(t1)",
      "sd x28, 224(t1)",
      "sd x29, 232(t1)",
      "sd x30, 240(t1)",
      "sd x31, 248(t1)",
      "frcsr t0",
      "sd t0, 256(t1)",
      "fsd f0, 264(t1)",
      "fsd f1, 272(t1)",
      "fsd f2, 280(t1)",
      "fsd f3, 288(t1)",
      "fsd f4, 296(t1)",
      "fsd f5, 304(t1)",
      "fsd f6, 312(t1)",
      "fsd f7, 320(t1)",
      "fsd f8, 328(t1)",
      "fsd f9, 336(t1)",
      "fsd f10, 344(t1)",
      "fsd f11, 352(t1)",
      "fsd f12, 360(t1)",
      "fsd f13, 368(t1)",
      "fsd f14, 376(t1)",
      "fsd f15, 384(t1)",
      "fsd f16, 392(t1)",
      "fsd f17, 400(t1)",
      "fsd f18, 408(t1)",
      "fsd f19, 416(t1)",
      "fsd f20, 424(t1)",
      "fsd f21, 432(t1)",
      "fsd f22, 440(t1)",
      "fsd f23, 448(t1)",
      "fsd f24, 456(t1)",
      "fsd f25, 464(t1)",
      "fsd f26, 472(t1)",
      "fsd f27, 480(t1)",
      "fsd f28, 488(t1)",
      "fsd f29, 496(t1)",
      "fsd f30, 504(t1)",
      "fsd f31, 512(t1)",
      "ld ra, 0(sp)",
      "ld s0, 8(sp)",
      "ld s1, 16(sp)",
      "ld s2, 24(sp)",
      "ld s3, 32(sp)",
      "ld s4, 40(sp)",
      "ld s5, 48(sp)",
      "ld s6, 56(sp)",
      "ld s7, 64(sp)",
      "ld s8, 72(sp)",
      "ld s9, 80(sp)",
      "ld s10, 88(sp)",
      "ld s11, 96(sp)",
      "fld fs0, 104(sp)",
      "fld fs1, 112(sp)",
      "fld fs2, 120(sp)",
      "fld fs3, 128(sp)",
      "fld fs4, 136(sp)",
      "fld fs5, 144(sp)",
      "fld fs6, 152(sp)",
      "fld fs7, 160(sp)",
      "fld fs8, 168(sp)",
      "fld fs9, 176(sp)",
      "fld fs10, 184(sp)",
      "fld fs11, 192(sp)",
      "addi sp, sp, 208",
      "ret",
    );
  }

  pub type Word = u64;
  /// There are no flags (FCSR is not part of the context).
  pub const FLAG: Word = 0;

  /// Returns the general-purpose registers of the context, by number.
  pub fn registers(context: &mut super::Context) -> (&mut [Word], Option<&mut Word>) {
    (&mut context.x, None)
  }

  /// Clobbers the floating-point registers.
  pub fn clobber_vectors() {
    // SAFETY: The clobbered registers are declared.
    unsafe {
      core::arch::asm!(
        "fmv.d.x f0, zero", "fmv.d.x f1, zero", "fmv.d.x f2, zero", "fmv.d.x f3, zero",
        "fmv.d.x f4, zero", "fmv.d.x f5, zero", "fmv.d.x f6, zero", "fmv.d.x f7, zero",
        "fmv.d.x f8, zero", "fmv.d.x f9, zero", "fmv.d.x f10, zero", "fmv.d.x f11, zero",
        "fmv.d.x f12, zero", "fmv.d.x f13, zero", "fmv.d.x f14, zero", "fmv.d.x f15, zero",
        "fmv.d.x f16, zero", "fmv.d.x f17, zero", "fmv.d.x f18, zero", "fmv.d.x f19, zero",
        "fmv.d.x f20, zero", "fmv.d.x f21, zero", "fmv.d.x f22, zero", "fmv.d.x f23, zero",
        "fmv.d.x f24, zero", "fmv.d.x f25, zero", "fmv.d.x f26, zero", "fmv.d.x f27, zero",
        "fmv.d.x f28, zero", "fmv.d.x f29, zero", "fmv.d.x f30, zero", "fmv.d.x f31, zero",
        out("f0") _, out("f1") _, out("f2") _, out("f3") _, out("f4") _, out("f5") _, out("f6") _, out("f7") _,
        out("f8") _, out("f9") _, out("f10") _, out("f11") _, out("f12") _, out("f13") _, out("f14") _, out("f15") _,
        out("f16") _, out("f17") _, out("f18") _, out("f19") _, out("f20") _, out("f21") _, out("f22") _, out("f23") _,
        out("f24") _, out("f25") _, out("f26") _, out("f27") _, out("f28") _, out("f29") _, out("f30") _, out("f31") _,
      );
    }
  }

  pub fn sp(context: &super::Context) -> u64 {
    context.sp()
  }
}

use probe::State;

/// Runs the probe, returning its output.
fn run(input: &State) -> State {
  let mut output = State::new(0);
  // SAFETY: Both states are valid.
  unsafe { probe::probe(input, &mut output) };
  output
}

/// Returns the address of the probe's hook point.
fn hook_point() -> *const () {
  run(&State::new(1)).point as usize as *const ()
}

/// Clobbers registers (including vector registers) in a handler.
#[inline(never)]
fn clobber(context: &Context) -> String {
  probe::clobber_vectors();
  let values: [f64; 8] = core::array::from_fn(|index| black_box(index as f64 * 1.5));
  let sum: f64 = values.iter().map(|value| value.sqrt()).sum();
  format!("{context:?} {sum}")
}

#[test]
fn preserves_state() -> Result<()> {
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  static CALLS: AtomicUsize = AtomicUsize::new(0);
  let input = State::new(0x5EED);
  let expected = run(&input);

  // SAFETY: The hook point consists of `NOP`s, which no code branches into.
  let detour = unsafe {
    MidDetour::new(hook_point(), |context| {
      black_box(clobber(context));
      CALLS.fetch_add(1, Ordering::SeqCst);
    })?
  };
  // SAFETY: No other thread executes the probe.
  unsafe { detour.enable()? };
  let actual = run(&input);
  // SAFETY: See above.
  unsafe { detour.disable()? };

  assert_eq!(CALLS.load(Ordering::SeqCst), 1);
  assert_eq!(actual.compared(), expected.compared());
  assert_eq!(run(&input).compared(), expected.compared());
  Ok(())
}

/// Tests the shim saving the extended state using FXSAVE (instead of XSAVE).
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[test]
fn preserves_state_with_fxsave() -> Result<()> {
  use super::arch::FORCE_FXSAVE;

  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  let input = State::new(0xF5A5);
  let expected = run(&input);

  FORCE_FXSAVE.store(true, Ordering::SeqCst);
  // SAFETY: See `preserves_state`.
  let detour = unsafe { MidDetour::new(hook_point(), |context| _ = black_box(clobber(context))) };
  FORCE_FXSAVE.store(false, Ordering::SeqCst);
  let detour = detour?;
  // SAFETY: No other thread executes the probe.
  unsafe { detour.enable()? };
  assert_eq!(run(&input).compared(), expected.compared());
  Ok(())
}

/// Where AVX is available, the shim must save its state using XSAVE.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[test]
fn saves_avx_state_if_available() {
  use super::arch::ExtendedState;

  // `preserves_state_with_fxsave` forces FXSAVE meanwhile
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());

  let avx = std::arch::is_x86_feature_detected!("avx");
  match ExtendedState::detect() {
    ExtendedState::Xsave { mask, .. } => assert_eq!(mask & 0b100 != 0, avx, "{mask:#x}"),
    ExtendedState::Fxsave => assert!(!avx),
  }
}

/// Tests that the upper halves of the YMM registers are preserved (by XSAVE),
/// even if the handler clears them.
#[cfg(target_arch = "x86_64")]
#[test]
fn preserves_avx_state() -> Result<()> {
  use core::arch::naked_asm;

  /// Loads YMM0 to YMM15 from `input`, executes the hook point, and stores
  /// them to `output`; returns the address of the hook point.
  ///
  /// AVX must be supported.
  #[unsafe(naked)]
  unsafe extern "sysv64" fn probe(input: *const [u128; 32], output: *mut [u128; 32]) -> usize {
    naked_asm!(
      "vmovdqu ymm0, [rdi]",
      "vmovdqu ymm1, [rdi + 32]",
      "vmovdqu ymm2, [rdi + 64]",
      "vmovdqu ymm3, [rdi + 96]",
      "vmovdqu ymm4, [rdi + 128]",
      "vmovdqu ymm5, [rdi + 160]",
      "vmovdqu ymm6, [rdi + 192]",
      "vmovdqu ymm7, [rdi + 224]",
      "vmovdqu ymm8, [rdi + 256]",
      "vmovdqu ymm9, [rdi + 288]",
      "vmovdqu ymm10, [rdi + 320]",
      "vmovdqu ymm11, [rdi + 352]",
      "vmovdqu ymm12, [rdi + 384]",
      "vmovdqu ymm13, [rdi + 416]",
      "vmovdqu ymm14, [rdi + 448]",
      "vmovdqu ymm15, [rdi + 480]",
      "2:",
      ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
      "vmovdqu [rsi], ymm0",
      "vmovdqu [rsi + 32], ymm1",
      "vmovdqu [rsi + 64], ymm2",
      "vmovdqu [rsi + 96], ymm3",
      "vmovdqu [rsi + 128], ymm4",
      "vmovdqu [rsi + 160], ymm5",
      "vmovdqu [rsi + 192], ymm6",
      "vmovdqu [rsi + 224], ymm7",
      "vmovdqu [rsi + 256], ymm8",
      "vmovdqu [rsi + 288], ymm9",
      "vmovdqu [rsi + 320], ymm10",
      "vmovdqu [rsi + 352], ymm11",
      "vmovdqu [rsi + 384], ymm12",
      "vmovdqu [rsi + 416], ymm13",
      "vmovdqu [rsi + 448], ymm14",
      "vmovdqu [rsi + 480], ymm15",
      "vzeroupper",
      "lea rax, [rip + 2b]",
      "ret",
    );
  }

  if !std::arch::is_x86_feature_detected!("avx") {
    return Ok(());
  }
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  let input: [u128; 32] = core::array::from_fn(|index| u128::from(index as u64 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101);
  let mut output = [0; 32];
  // SAFETY: AVX is supported, and both arrays are valid.
  let point = unsafe { probe(&input, &mut output) };
  assert_eq!(output, input);

  let handler = |_: &mut Context| {
    // SAFETY: AVX is supported, and the cleared registers are declared.
    unsafe {
      core::arch::asm!(
        "vzeroall",
        out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
        out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
        out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
        out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
      );
    }
  };
  // SAFETY: See `preserves_state`.
  let detour = unsafe { MidDetour::new(point as *const (), handler)? };
  // SAFETY: No other thread executes the probe.
  unsafe { detour.enable()? };
  output = [0; 32];
  // SAFETY: See above.
  unsafe { probe(&input, &mut output) };
  assert_eq!(output, input);
  Ok(())
}

/// Returns the registers of [`State::r`] that are compared (and loaded).
fn compared_registers() -> Vec<usize> {
  let compared = State::new(1).compared();
  (0..compared.r.len()).filter(|&index| compared.r[index] != 0).collect()
}

/// Tests that the context holds the registers, and that modifying any of them
/// (and the flags) is applied.
// The stack pointer is 32-bit on x86
#[allow(clippy::unnecessary_cast)]
#[test]
fn applies_modifications() -> Result<()> {
  static SP: AtomicU64 = AtomicU64::new(0);
  static MISMATCHES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  let input = State::new(0xC0DE);
  let registers = compared_registers();
  let baseline = run(&input);

  let handler = {
    let registers = registers.clone();
    move |context: &mut Context| {
      SP.store(probe::sp(context) as u64, Ordering::SeqCst);
      let (values, flags) = probe::registers(context);
      for &index in &registers {
        if values[index] != input.r[index] {
          MISMATCHES.lock().unwrap().push(index);
        }
        values[index] = !values[index];
      }
      if let Some(flags) = flags {
        *flags ^= probe::FLAG;
      }
    }
  };
  // SAFETY: See `preserves_state`.
  let detour = unsafe { MidDetour::new(hook_point(), handler)? };
  // SAFETY: No other thread executes the probe.
  unsafe { detour.enable()? };
  let actual = run(&input);

  let mut expected = baseline;
  for &index in &registers {
    expected.r[index] = !input.r[index];
  }
  expected.flags ^= probe::FLAG;
  assert_eq!(*MISMATCHES.lock().unwrap(), []);
  assert_eq!(actual.compared(), expected.compared());
  assert_eq!(SP.load(Ordering::SeqCst), actual.sp as u64);
  Ok(())
}

/// Tests that a closure may drop its own detour (whose generated code it
/// returns to).
#[test]
fn closure_drops_its_detour() -> Result<()> {
  static DETOUR: Mutex<Option<MidDetour>> = Mutex::new(None);
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  let input = State::new(0xD809);
  let expected = run(&input);

  // SAFETY: See `preserves_state`.
  let detour = unsafe {
    MidDetour::new(hook_point(), |context| {
      black_box(clobber(context));
      drop(DETOUR.lock().unwrap().take());
    })?
  };
  // SAFETY: No other thread executes the probe.
  unsafe { detour.enable()? };
  *DETOUR.lock().unwrap() = Some(detour);

  assert_eq!(run(&input).compared(), expected.compared());
  assert!(DETOUR.lock().unwrap().is_none());
  assert_eq!(run(&input).compared(), expected.compared());
  Ok(())
}

/// Tests that the closure starts with an empty x87 register stack, and that
/// the x87 state of the target is preserved.
#[cfg(target_arch = "x86_64")]
#[test]
fn preserves_x87_state() -> Result<()> {
  use core::arch::naked_asm;

  /// Loads 8 values from `input` onto the x87 register stack, executes the
  /// hook point, and stores them to `output`; returns the address of the
  /// hook point.
  #[unsafe(naked)]
  unsafe extern "sysv64" fn probe(input: *const [f64; 8], output: *mut [f64; 8]) -> usize {
    naked_asm!(
      "fld qword ptr [rdi]",
      "fld qword ptr [rdi + 8]",
      "fld qword ptr [rdi + 16]",
      "fld qword ptr [rdi + 24]",
      "fld qword ptr [rdi + 32]",
      "fld qword ptr [rdi + 40]",
      "fld qword ptr [rdi + 48]",
      "fld qword ptr [rdi + 56]",
      "2:",
      ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
      "fstp qword ptr [rsi]",
      "fstp qword ptr [rsi + 8]",
      "fstp qword ptr [rsi + 16]",
      "fstp qword ptr [rsi + 24]",
      "fstp qword ptr [rsi + 32]",
      "fstp qword ptr [rsi + 40]",
      "fstp qword ptr [rsi + 48]",
      "fstp qword ptr [rsi + 56]",
      "lea rax, [rip + 2b]",
      "ret",
    );
  }

  static ONE: Mutex<f64> = Mutex::new(0.0);
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  let input: [f64; 8] = core::array::from_fn(|index| index as f64 + 0.5);
  let mut expected = [0.0; 8];
  // SAFETY: Both arrays are valid, and the x87 stack is empty (as required).
  let point = unsafe { probe(&input, &mut expected) };

  let handler = |_: &mut Context| {
    let mut one = 0.0f64;
    // SAFETY: Uses one x87 register (balanced), which requires an empty stack.
    unsafe { core::arch::asm!("fld1", "fstp qword ptr [{one}]", one = in(reg) &raw mut one) };
    *ONE.lock().unwrap() = one;
  };
  // SAFETY: See `preserves_state`.
  let detour = unsafe { MidDetour::new(point as *const (), handler)? };
  // SAFETY: No other thread executes the probe.
  unsafe { detour.enable()? };
  let mut actual = [0.0; 8];
  // SAFETY: See above.
  unsafe { probe(&input, &mut actual) };

  assert_eq!(*ONE.lock().unwrap(), 1.0);
  assert_eq!(actual, expected);
  Ok(())
}

#[test]
fn chains_with_other_detours() -> Result<()> {
  static ORDER: Mutex<Vec<u8>> = Mutex::new(Vec::new());
  let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
  let input = State::new(0xC4A1);
  let point = hook_point();

  // SAFETY: See `preserves_state`.
  let (first, second) = unsafe {
    (
      MidDetour::new(point, |_| ORDER.lock().unwrap().push(1))?,
      MidDetour::new(point, |_| ORDER.lock().unwrap().push(2))?,
    )
  };
  let mut transaction = crate::Transaction::new();
  transaction.enable(&first).enable(&second);
  // SAFETY: No other thread executes the probe.
  unsafe { transaction.commit(crate::Threads::None)? };
  let actual = run(&input);

  // The most recently enabled detour is called first
  assert_eq!(*ORDER.lock().unwrap(), [2, 1]);
  assert_eq!(actual.compared(), run_without(&input, [&first, &second])?.compared());
  Ok(())
}

/// Runs the probe with `detours` disabled.
fn run_without<const N: usize>(input: &State, detours: [&MidDetour; N]) -> Result<State> {
  for detour in detours {
    // SAFETY: No other thread executes the probe.
    unsafe { detour.disable()? };
  }
  Ok(run(input))
}
