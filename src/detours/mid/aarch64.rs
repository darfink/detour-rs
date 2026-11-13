//! The shim of mid-function detours on AArch64.
//!
//! The shim skips the red zone (128 bytes below the stack pointer, which leaf
//! functions may use on Apple platforms), and allocates a frame holding:
//!
//! | Offset | Contents                                  |
//! |--------|-------------------------------------------|
//! | 0      | X0 to X30, SP & NZCV (the [`Context`])    |
//! | 264    | FPSR                                      |
//! | 272    | FPCR                                      |
//! | 288    | Q0 to Q31                                 |
//! | 800    | (end)                                     |
//!
//! It calls [`dispatch`], restores everything (except X16, the scratch
//! register), and jumps to the trampoline through X16. SVE & SME state beyond
//! the Q registers is not saved; Rust code does not use it unless enabled.

use super::{Closure, Shim};
use alloc::vec;
use alloc::vec::Vec;

/// The general-purpose registers & flags at the target of a [`MidDetour`].
///
/// Changes of the registers & flags are applied when execution continues,
/// except for the stack pointer (which is read-only) and X16 (the scratch
/// register, which holds no value at the target).
///
/// [`MidDetour`]: super::MidDetour
#[repr(C)]
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
  /// X0 to X30 (the link register).
  pub x: [u64; 31],
  sp: u64,
  /// The condition flags, as read by `MRS Xt, NZCV`.
  pub nzcv: u64,
}

impl Context {
  /// Returns the stack pointer at the target.
  pub fn sp(&self) -> u64 {
    self.sp
  }
}

/// Invokes the handler; called by the shim.
pub(super) unsafe extern "C" fn dispatch(context: *mut Context, closure: *const Closure) {
  // SAFETY: The shim passes its saved context, and the closure of its detour.
  unsafe { super::invoke(context, closure) }
}

const RED_ZONE: u32 = 128;
const FRAME: u32 = 800;
/// NZCV, followed by FPSR.
const NZCV: u32 = 256;
const FPCR: u32 = 272;
const VECTORS: u32 = 288;
const X16: u32 = 16;
const X17: u32 = 17;
const SP: u32 = 31;

/// `STP Xt, Xt2, [SP, #offset]` (or `LDP`).
fn pair(load: bool, rt: u32, rt2: u32, offset: u32) -> u32 {
  let opcode = if load { 0xA940_0000 } else { 0xA900_0000 };
  opcode | (offset / 8) << 15 | rt2 << 10 | SP << 5 | rt
}

/// `STP Qt, Qt2, [SP, #offset]` (or `LDP`).
fn vector_pair(load: bool, rt: u32, rt2: u32, offset: u32) -> u32 {
  let opcode = if load { 0xAD40_0000 } else { 0xAD00_0000 };
  opcode | (offset / 16) << 15 | rt2 << 10 | SP << 5 | rt
}

/// `STR Xt, [SP, #offset]` (or `LDR`).
fn single(load: bool, rt: u32, offset: u32) -> u32 {
  let opcode = if load { 0xF940_0000 } else { 0xF900_0000 };
  opcode | (offset / 8) << 10 | SP << 5 | rt
}

/// `ADD Xd, SP, #imm` (`Xd` may be SP).
fn add_sp(rd: u32, imm: u32) -> u32 {
  0x9100_0000 | imm << 10 | SP << 5 | rd
}

/// `SUB SP, SP, #imm`
fn sub_sp(imm: u32) -> u32 {
  0xD100_0000 | imm << 10 | SP << 5 | SP
}

/// `LDR Xt, <pc + offset>`
fn ldr_literal(rt: u32, offset: u32) -> u32 {
  0x5800_0000 | (offset / 4) << 5 | rt
}

// `MRS Xt, <register>` & `MSR <register>, Xt`, without `Xt`
const MRS_NZCV: u32 = 0xD53B_4200;
const MSR_NZCV: u32 = 0xD51B_4200;
const MRS_FPSR: u32 = 0xD53B_4420;
const MSR_FPSR: u32 = 0xD51B_4420;
const MRS_FPCR: u32 = 0xD53B_4400;
const MSR_FPCR: u32 = 0xD51B_4400;

/// Generates the shim.
pub(super) fn shim(shim: &Shim) -> Vec<u8> {
  let mut code = Vec::new();
  code.push(sub_sp(RED_ZONE + FRAME));
  for register in (0..30).step_by(2) {
    code.push(pair(false, register, register + 1, register * 8));
  }
  // The stack pointer at the target, along with X30
  code.push(add_sp(X16, RED_ZONE + FRAME));
  code.push(pair(false, 30, X16, 240));
  code.push(MRS_NZCV | X16);
  code.push(MRS_FPSR | X17);
  code.push(pair(false, X16, X17, NZCV));
  code.push(MRS_FPCR | X16);
  code.push(single(false, X16, FPCR));
  for register in (0..32).step_by(2) {
    code.push(vector_pair(false, register, register + 1, VECTORS + register * 16));
  }

  // MOV X0, SP; LDR X1, <handler>; LDR X16, <dispatch>; BLR X16
  // (the literal loads are completed below, as (index, register, literal))
  code.push(add_sp(0, 0));
  let mut loads = vec![(code.len(), 1, 0)];
  code.push(0);
  loads.push((code.len(), X16, 1));
  code.push(0);
  code.push(0xD63F_0000 | X16 << 5);

  code.push(single(true, X16, FPCR));
  code.push(MSR_FPCR | X16);
  code.push(pair(true, X16, X17, NZCV));
  code.push(MSR_NZCV | X16);
  code.push(MSR_FPSR | X17);
  for register in (0..32).step_by(2) {
    code.push(vector_pair(true, register, register + 1, VECTORS + register * 16));
  }
  code.push(single(true, 30, 240));
  for register in (0..30).step_by(2) {
    if register == X16 {
      // X16 is overwritten below
      code.push(single(true, X17, X17 * 8));
    } else {
      code.push(pair(true, register, register + 1, register * 8));
    }
  }
  code.push(add_sp(SP, RED_ZONE + FRAME));
  // LDR X16, <trampoline>; BR X16
  loads.push((code.len(), X16, 2));
  code.push(0);
  code.push(0xD61F_0000 | X16 << 5);

  // The literals follow the code, 8-byte aligned (padded with `UDF #0`)
  if code.len() % 2 != 0 {
    code.push(0);
  }
  let literals = code.len();
  for (index, register, literal) in loads {
    code[index] = ldr_literal(register, 4 * (literals + 2 * literal - index) as u32);
  }
  let mut bytes: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
  for literal in [shim.handler, shim.dispatch, shim.trampoline] {
    bytes.extend_from_slice(&(literal as u64).to_le_bytes());
  }
  bytes
}
