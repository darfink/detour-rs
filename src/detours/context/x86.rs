//! The shim of mid-function detours on x86 & x86-64.
//!
//! The shim saves the flags and the accumulator at the target's stack pointer
//! (which need not be aligned), aligns the stack, and pushes the original
//! stack pointer and the general-purpose registers & flags above it (forming
//! the [`Context`], which is therefore aligned). It then clears the direction
//! flag, saves the extended state (XSAVE,
//! or FXSAVE if unsupported) on a 64-byte aligned stack, initializes the x87
//! state (emptying its register stack, which calls require), and calls
//! [`dispatch`]. It then restores the extended state, the registers (moved
//! from the context) and the flags, pops the original stack pointer, and jumps
//! to the trampoline.
//!
//! On x86-64, the red zone (128 bytes below the stack pointer, which leaf
//! functions may use on System V) is skipped first. The extended state of AMX
//! (whose tiles are several KiB) is not saved, keeping the stack usage within
//! a page (for stack probing on Windows); Rust code does not use it.

use super::{Closure, Shim};
use alloc::vec::Vec;

/// The general-purpose registers & flags at the target of a [`ContextDetour`].
///
/// Changes of the registers & flags are applied when execution continues,
/// except for the stack pointer, which is read-only.
///
/// [`ContextDetour`]: super::ContextDetour
#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
  /// `RAX`
  pub rax: u64,
  /// `RCX`
  pub rcx: u64,
  /// `RDX`
  pub rdx: u64,
  /// `RBX`
  pub rbx: u64,
  rsp: u64,
  /// `RBP`
  pub rbp: u64,
  /// `RSI`
  pub rsi: u64,
  /// `RDI`
  pub rdi: u64,
  /// `R8`
  pub r8: u64,
  /// `R9`
  pub r9: u64,
  /// `R10`
  pub r10: u64,
  /// `R11`
  pub r11: u64,
  /// `R12`
  pub r12: u64,
  /// `R13`
  pub r13: u64,
  /// `R14`
  pub r14: u64,
  /// `R15`
  pub r15: u64,
  /// `RFLAGS`
  pub rflags: u64,
}

#[cfg(target_arch = "x86_64")]
impl Context {
  /// Returns the stack pointer at the target.
  pub fn rsp(&self) -> u64 {
    self.rsp
  }
}

/// The general-purpose registers & flags at the target of a [`ContextDetour`].
///
/// Changes of the registers & flags are applied when execution continues,
/// except for the stack pointer, which is read-only.
///
/// [`ContextDetour`]: super::ContextDetour
#[cfg(target_arch = "x86")]
#[repr(C)]
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
  /// `EAX`
  pub eax: u32,
  /// `ECX`
  pub ecx: u32,
  /// `EDX`
  pub edx: u32,
  /// `EBX`
  pub ebx: u32,
  esp: u32,
  /// `EBP`
  pub ebp: u32,
  /// `ESI`
  pub esi: u32,
  /// `EDI`
  pub edi: u32,
  /// `EFLAGS`
  pub eflags: u32,
}

#[cfg(target_arch = "x86")]
impl Context {
  /// Returns the stack pointer at the target.
  pub fn esp(&self) -> u32 {
    self.esp
  }
}

/// Invokes the handler; called by the shim.
#[cfg(target_arch = "x86_64")]
pub(super) unsafe extern "sysv64" fn dispatch(context: *mut Context, closure: *const Closure) {
  // SAFETY: The shim passes its saved context, and the closure of its detour.
  unsafe { super::invoke(context, closure) }
}

/// Invokes the handler; called by the shim.
#[cfg(target_arch = "x86")]
pub(super) unsafe extern "C" fn dispatch(context: *mut Context, closure: *const Closure) {
  // SAFETY: The shim passes its saved context, and the closure of its detour.
  unsafe { super::invoke(context, closure) }
}

/// How the extended state (x87, SSE, AVX, etc.) is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExtendedState {
  /// FXSAVE (x87 & SSE), 512 bytes.
  Fxsave,
  /// XSAVE of the components in `mask`, requiring `size` bytes.
  Xsave { mask: u64, size: u32 },
}

/// Forces FXSAVE, to test it on processors supporting XSAVE.
#[cfg(test)]
pub(super) static FORCE_FXSAVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The components of AMX (`TILECFG` & `TILEDATA`), which are not saved.
const AMX_COMPONENTS: u64 = 0b11 << 17;

impl ExtendedState {
  /// Returns how the extended state is saved on this processor.
  pub(super) fn detect() -> Self {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::__cpuid_count;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::__cpuid_count;

    // CPUID.01H:ECX.OSXSAVE (the OS has enabled XSAVE)
    // SAFETY: CPUID is available on all supported processors (and is a safe
    // function in later versions of Rust).
    #[allow(unused_unsafe)]
    let features = unsafe { __cpuid_count(1, 0) };
    #[cfg(test)]
    if FORCE_FXSAVE.load(core::sync::atomic::Ordering::SeqCst) {
      return ExtendedState::Fxsave;
    }
    if features.ecx & (1 << 27) == 0 {
      return ExtendedState::Fxsave;
    }

    let mask = xcr0() & !AMX_COMPONENTS;
    // The legacy region and the XSAVE header, followed by the components
    let mut size = 512 + 64;
    for component in 2..64 {
      if mask & (1 << component) != 0 {
        // SAFETY: See above (leaf 0DH is supported, since XSAVE is).
        #[allow(unused_unsafe)]
        let leaf = unsafe { __cpuid_count(0xD, component) };
        size = size.max(leaf.ebx + leaf.eax);
      }
    }
    ExtendedState::Xsave { mask, size }
  }

  /// Returns the size of the save area, a multiple of 64 bytes.
  fn size(self) -> u32 {
    match self {
      ExtendedState::Fxsave => 512,
      ExtendedState::Xsave { size, .. } => size.next_multiple_of(64),
    }
  }
}

/// Returns the value of `XCR0`, the enabled XSAVE components.
fn xcr0() -> u64 {
  let (low, high): (u32, u32);
  // SAFETY: XGETBV is supported, since the OS has enabled XSAVE.
  unsafe {
    core::arch::asm!("xgetbv", in("ecx") 0, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
  }
  u64::from(high) << 32 | u64::from(low)
}

/// Emits x86 code.
struct Emitter(Vec<u8>);

impl Emitter {
  fn bytes(&mut self, bytes: &[u8]) {
    self.0.extend_from_slice(bytes);
  }

  /// Loads the XSAVE mask into EDX:EAX.
  fn mask(&mut self, mask: u64) {
    // mov eax, low; mov edx, high
    self.bytes(&[0xB8]);
    self.bytes(&(mask as u32).to_le_bytes());
    self.bytes(&[0xBA]);
    self.bytes(&((mask >> 32) as u32).to_le_bytes());
  }
}

/// Generates the shim.
pub(super) fn shim(shim: &Shim) -> Vec<u8> {
  generate(shim, ExtendedState::detect())
}

#[cfg(target_arch = "x86_64")]
fn generate(shim: &Shim, state: ExtendedState) -> Vec<u8> {
  let mut code = Emitter(Vec::new());
  // Save the flags & RAX below the red zone, at the (possibly misaligned) stack
  // pointer of the target: lea rsp, [rsp - 128]; pushfq; push rax;
  // lea rax, [rsp + 144] (the original RSP); and rsp, -16
  code.bytes(&[0x48, 0x8D, 0x64, 0x24, 0x80, 0x9C, 0x50]);
  code.bytes(&[0x48, 0x8D, 0x84, 0x24, 0x90, 0x00, 0x00, 0x00, 0x48, 0x83, 0xE4, 0xF0]);
  // Push the original RSP (popped last), and the context above it:
  // push rax; push qword ptr [rax - 136] (the flags)
  code.bytes(&[0x50, 0xFF, 0xB0, 0x78, 0xFF, 0xFF, 0xFF]);
  // push r15 .. r8
  for register in (0..8).rev() {
    code.bytes(&[0x41, 0x50 + register]);
  }
  // push rdi, rsi, rbp, rax (the original RSP), rbx, rdx, rcx;
  // push qword ptr [rax - 144] (RAX)
  code.bytes(&[0x57, 0x56, 0x55, 0x50, 0x53, 0x52, 0x51, 0xFF, 0xB0, 0x70, 0xFF, 0xFF, 0xFF]);
  // cld; mov rbx, rsp; and rsp, -64; sub rsp, size
  code.bytes(&[0xFC, 0x48, 0x89, 0xE3, 0x48, 0x83, 0xE4, 0xC0, 0x48, 0x81, 0xEC]);
  code.bytes(&state.size().to_le_bytes());
  match state {
    ExtendedState::Fxsave => code.bytes(&[0x48, 0x0F, 0xAE, 0x04, 0x24]), // fxsave64 [rsp]
    ExtendedState::Xsave { mask, .. } => {
      // Zero the XSAVE header: xor eax, eax; mov [rsp + 512 + 8 * i], rax
      code.bytes(&[0x31, 0xC0]);
      for offset in (512..576).step_by(8) {
        code.bytes(&[0x48, 0x89, 0x84, 0x24]);
        code.bytes(&(offset as u32).to_le_bytes());
      }
      code.mask(mask);
      code.bytes(&[0x48, 0x0F, 0xAE, 0x24, 0x24]); // xsave64 [rsp]
    },
  }
  // Empty the x87 register stack (saved above), as calls require: fninit
  code.bytes(&[0xDB, 0xE3]);
  // mov rdi, rbx; mov rsi, closure; mov rax, dispatch; call rax
  code.bytes(&[0x48, 0x89, 0xDF, 0x48, 0xBE]);
  code.bytes(&(shim.handler as u64).to_le_bytes());
  code.bytes(&[0x48, 0xB8]);
  code.bytes(&(shim.dispatch as u64).to_le_bytes());
  code.bytes(&[0xFF, 0xD0]);
  match state {
    ExtendedState::Fxsave => code.bytes(&[0x48, 0x0F, 0xAE, 0x0C, 0x24]), // fxrstor64 [rsp]
    ExtendedState::Xsave { mask, .. } => {
      code.mask(mask);
      code.bytes(&[0x48, 0x0F, 0xAE, 0x2C, 0x24]); // xrstor64 [rsp]
    },
  }
  // mov rsp, rbx; then restore the registers from the context, all at or above
  // the stack pointer (except RSP): mov <register>, [rsp + 8 * number]
  code.bytes(&[0x48, 0x89, 0xDC, 0x48, 0x8B, 0x04, 0x24]);
  for (prefix, register) in [(0x48, 1), (0x48, 2), (0x48, 5), (0x48, 6), (0x48, 7)]
    .into_iter()
    .chain((8..16).map(|register| (0x4C, register)))
  {
    let modrm = 0x44 | (register & 7) << 3;
    code.bytes(&[prefix, 0x8B, modrm, 0x24, 8 * register]);
  }
  code.bytes(&[0x48, 0x8B, 0x5C, 0x24, 0x18]); // mov rbx, [rsp + 24]
  // lea rsp, [rsp + 128]; popfq; pop rsp (the original RSP);
  // jmp [rip]; .quad trampoline
  code.bytes(&[0x48, 0x8D, 0xA4, 0x24, 0x80, 0x00, 0x00, 0x00, 0x9D, 0x5C, 0xFF, 0x25, 0, 0, 0, 0]);
  code.bytes(&(shim.trampoline as u64).to_le_bytes());
  code.0
}

#[cfg(target_arch = "x86")]
fn generate(shim: &Shim, state: ExtendedState) -> Vec<u8> {
  let mut code = Emitter(Vec::new());
  // Save the flags & EAX at the (possibly misaligned) stack pointer of the
  // target: pushfd; push eax; lea eax, [esp + 8] (the original ESP);
  // and esp, -16
  code.bytes(&[0x9C, 0x50, 0x8D, 0x44, 0x24, 0x08, 0x83, 0xE4, 0xF0]);
  // Push the original ESP (popped last), and the context above it: push eax;
  // push dword ptr [eax - 4] (the flags); push edi, esi, ebp, eax (the
  // original ESP), ebx, edx, ecx; push dword ptr [eax - 8] (EAX)
  code.bytes(&[0x50, 0xFF, 0x70, 0xFC, 0x57, 0x56, 0x55, 0x50, 0x53, 0x52, 0x51, 0xFF, 0x70, 0xF8]);
  // cld; mov ebx, esp; and esp, -64; sub esp, size
  code.bytes(&[0xFC, 0x89, 0xE3, 0x83, 0xE4, 0xC0, 0x81, 0xEC]);
  code.bytes(&state.size().to_le_bytes());
  match state {
    ExtendedState::Fxsave => code.bytes(&[0x0F, 0xAE, 0x04, 0x24]), // fxsave [esp]
    ExtendedState::Xsave { mask, .. } => {
      // Zero the XSAVE header: xor eax, eax; mov [esp + 512 + 4 * i], eax
      code.bytes(&[0x31, 0xC0]);
      for offset in (512..576).step_by(4) {
        code.bytes(&[0x89, 0x84, 0x24]);
        code.bytes(&(offset as u32).to_le_bytes());
      }
      code.mask(mask);
      code.bytes(&[0x0F, 0xAE, 0x24, 0x24]); // xsave [esp]
    },
  }
  // Empty the x87 register stack (saved above), as calls require: fninit
  code.bytes(&[0xDB, 0xE3]);
  // Keeps the stack 16-byte aligned at the call:
  // sub esp, 8; push closure; push ebx; mov eax, dispatch; call eax; add esp, 16
  code.bytes(&[0x83, 0xEC, 0x08, 0x68]);
  code.bytes(&(shim.handler as u32).to_le_bytes());
  code.bytes(&[0x53, 0xB8]);
  code.bytes(&(shim.dispatch as u32).to_le_bytes());
  code.bytes(&[0xFF, 0xD0, 0x83, 0xC4, 0x10]);
  match state {
    ExtendedState::Fxsave => code.bytes(&[0x0F, 0xAE, 0x0C, 0x24]), // fxrstor [esp]
    ExtendedState::Xsave { mask, .. } => {
      code.mask(mask);
      code.bytes(&[0x0F, 0xAE, 0x2C, 0x24]); // xrstor [esp]
    },
  }
  // mov esp, ebx; then restore the registers from the context, all at or above
  // the stack pointer (except ESP): mov eax, [esp]; mov <register>, [esp + 4 *
  // number] (ECX, EDX, EBP, ESI, EDI & EBX)
  code.bytes(&[0x89, 0xDC, 0x8B, 0x04, 0x24]);
  for register in [1, 2, 5, 6, 7, 3] {
    code.bytes(&[0x8B, 0x44 | register << 3, 0x24, 4 * register]);
  }
  // lea esp, [esp + 32]; popfd; pop esp (the original ESP);
  // jmp [literal]; .long trampoline
  code.bytes(&[0x8D, 0x64, 0x24, 0x20, 0x9D, 0x5C]);
  let literal = shim.base + code.0.len() + 6;
  code.bytes(&[0xFF, 0x25]);
  code.bytes(&(literal as u32).to_le_bytes());
  code.bytes(&(shim.trampoline as u32).to_le_bytes());
  code.0
}
