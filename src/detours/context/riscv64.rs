//! The shim of mid-function detours on RISC-V (RV64).
//!
//! There is no red zone in the psABI, so the shim allocates a frame directly
//! below the stack pointer, aligned to 16 bytes, holding:
//!
//! | Offset | Contents                                  |
//! |--------|-------------------------------------------|
//! | 0      | X0 to X31 (the [`Context`])               |
//! | 256    | F0 to F31                                 |
//! | 512    | FCSR                                      |
//! | 520    | The stack pointer at the target           |
//! | 528    | (end)                                     |
//!
//! It calls [`dispatch`], restores everything (except T1, the scratch
//! register), and jumps to the trampoline through T1. The state of the vector
//! extension (V) is not saved; Rust code does not use it unless enabled.

use super::{Closure, Shim};
use alloc::vec;
use alloc::vec::Vec;

/// The general-purpose registers at the target of a [`ContextDetour`].
///
/// Changes of the registers are applied when execution continues, except for
/// X0 (the zero register), X2 (the stack pointer, which is read-only) and X6
/// (T1, the scratch register, which holds no value at the target and reads
/// as zero).
///
/// [`ContextDetour`]: super::ContextDetour
#[repr(C)]
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
  /// X0 to X31, indexed by register number (e.g. `x[10]` is A0).
  pub x: [u64; 32],
}

impl Context {
  /// Returns the stack pointer at the target.
  pub fn sp(&self) -> u64 {
    self.x[SP as usize]
  }
}

/// Invokes the handler; called by the shim.
pub(super) unsafe extern "C" fn dispatch(context: *mut Context, closure: *const Closure) {
  // SAFETY: The shim passes its saved context, and the closure of its detour.
  unsafe { super::invoke(context, closure) }
}

const FRAME: u32 = 528;
const FLOATS: u32 = 256;
const FCSR: u32 = 512;
/// The stack pointer at the target, which is restored (unlike `x[2]`).
const ORIGINAL_SP: u32 = 520;

const ZERO: u32 = 0;
const RA: u32 = 1;
const SP: u32 = 2;
const T1: u32 = 6;
const A0: u32 = 10;
const A1: u32 = 11;

/// The CSR number of FCSR.
const CSR_FCSR: u32 = 0x003;

/// An I-type instruction: `<op> rd, imm(rs1)` (or `<op> rd, rs1, imm`).
fn i_type(opcode: u32, funct3: u32, rd: u32, rs1: u32, imm: i32) -> u32 {
  debug_assert!((-2048..2048).contains(&imm));
  (imm as u32 & 0xFFF) << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | opcode
}

/// An S-type instruction: `<op> rs2, imm(rs1)`.
fn s_type(opcode: u32, funct3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 {
  debug_assert!((-2048..2048).contains(&imm));
  let imm = imm as u32 & 0xFFF;
  (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | (imm & 0x1F) << 7 | opcode
}

/// `SD rs2, offset(SP)` (or `LD rs2, offset(SP)`).
fn single(load: bool, register: u32, offset: u32) -> u32 {
  if load {
    i_type(0x03, 0b011, register, SP, offset as i32)
  } else {
    s_type(0x23, 0b011, SP, register, offset as i32)
  }
}

/// `FSD frs2, offset(SP)` (or `FLD frs2, offset(SP)`).
fn float(load: bool, register: u32, offset: u32) -> u32 {
  if load {
    i_type(0x07, 0b011, register, SP, offset as i32)
  } else {
    s_type(0x27, 0b011, SP, register, offset as i32)
  }
}

/// `ADDI rd, rs1, imm`
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
  i_type(0x13, 0b000, rd, rs1, imm)
}

/// `ANDI rd, rs1, imm`
fn andi(rd: u32, rs1: u32, imm: i32) -> u32 {
  i_type(0x13, 0b111, rd, rs1, imm)
}

/// `LD rd, imm(rs1)`
fn ld(rd: u32, rs1: u32, imm: i32) -> u32 {
  i_type(0x03, 0b011, rd, rs1, imm)
}

/// `JALR rd, imm(rs1)`
fn jalr(rd: u32, rs1: u32, imm: i32) -> u32 {
  i_type(0x67, 0b000, rd, rs1, imm)
}

/// `AUIPC rd, imm`
fn auipc(rd: u32, imm: i32) -> u32 {
  (imm as u32 & 0xF_FFFF) << 12 | rd << 7 | 0x17
}

/// `CSRRS rd, FCSR, X0` (`FRCSR rd`).
fn read_fcsr(rd: u32) -> u32 {
  CSR_FCSR << 20 | 0b010 << 12 | rd << 7 | 0x73
}

/// `CSRRW X0, FCSR, rs1` (`FSCSR rs1`).
fn write_fcsr(rs1: u32) -> u32 {
  CSR_FCSR << 20 | rs1 << 15 | 0b001 << 12 | 0x73
}

/// Generates the shim.
pub(super) fn shim(shim: &Shim) -> Vec<u8> {
  let mut code = vec![
    // MV T1, SP; ADDI SP, SP, -FRAME; ANDI SP, SP, -16
    addi(T1, SP, 0),
    addi(SP, SP, -(FRAME as i32)),
    andi(SP, SP, -16),
    single(false, T1, ORIGINAL_SP),
    single(false, T1, SP * 8),
    single(false, ZERO, 0),
    single(false, ZERO, T1 * 8),
  ];
  for register in (1..32).filter(|&register| register != SP && register != T1) {
    code.push(single(false, register, register * 8));
  }
  code.push(read_fcsr(T1));
  code.push(single(false, T1, FCSR));
  for register in 0..32 {
    code.push(float(false, register, FLOATS + register * 8));
  }

  // MV A0, SP; LD A1, <handler>; LD T1, <dispatch>; JALR RA, T1
  // (the literal loads, `AUIPC` & `LD`, are completed below, as (index,
  // register, literal))
  code.push(addi(A0, SP, 0));
  let mut loads = vec![(code.len(), A1, 0)];
  code.extend([0, 0]);
  loads.push((code.len(), T1, 1));
  code.extend([0, 0]);
  code.push(jalr(RA, T1, 0));

  code.push(single(true, T1, FCSR));
  code.push(write_fcsr(T1));
  for register in 0..32 {
    code.push(float(true, register, FLOATS + register * 8));
  }
  for register in (1..32).filter(|&register| register != SP && register != T1) {
    code.push(single(true, register, register * 8));
  }
  // LD T1, <trampoline>; LD SP, ORIGINAL_SP(SP); JR T1
  loads.push((code.len(), T1, 2));
  code.extend([0, 0]);
  code.push(single(true, SP, ORIGINAL_SP));
  code.push(jalr(ZERO, T1, 0));

  // The literals follow the code, 8-byte aligned (padded with `UNIMP`)
  if code.len() % 2 != 0 {
    code.push(0);
  }
  let literals = code.len();
  for (index, register, literal) in loads {
    let offset = 4 * (literals + 2 * literal - index) as i32;
    let (hi, lo) = ((offset + 0x800) >> 12, (offset + 0x800).rem_euclid(0x1000) - 0x800);
    code[index] = auipc(register, hi);
    code[index + 1] = ld(register, register, lo);
  }
  let mut bytes: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
  for literal in [shim.handler, shim.dispatch, shim.trampoline] {
    bytes.extend_from_slice(&(literal as u64).to_le_bytes());
  }
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Compares the generated encodings with those of the LLVM assembler.
  #[test]
  fn encodes_instructions() {
    assert_eq!(addi(T1, SP, 0), 0x0001_0313); // mv t1, sp
    assert_eq!(addi(SP, SP, -528), 0xDF01_0113); // addi sp, sp, -528
    assert_eq!(andi(SP, SP, -16), 0xFF01_7113); // andi sp, sp, -16
    assert_eq!(single(false, T1, 520), 0x2061_3423); // sd t1, 520(sp)
    assert_eq!(single(false, ZERO, 0), 0x0001_3023); // sd zero, 0(sp)
    assert_eq!(single(true, SP, 520), 0x2081_3103); // ld sp, 520(sp)
    assert_eq!(float(false, 31, 504), 0x1FF1_3C27); // fsd ft11, 504(sp)
    assert_eq!(float(true, 31, 504), 0x1F81_3F87); // fld ft11, 504(sp)
    assert_eq!(read_fcsr(T1), 0x0030_2373); // frcsr t1
    assert_eq!(write_fcsr(T1), 0x0033_1073); // fscsr t1
    assert_eq!(auipc(A1, 0), 0x0000_0597); // auipc a1, 0
    assert_eq!(ld(A1, A1, 0x7F8), 0x7F85_B583); // ld a1, 2040(a1)
    assert_eq!(ld(A1, A1, -8), 0xFF85_B583); // ld a1, -8(a1)
    assert_eq!(jalr(RA, T1, 0), 0x0003_00E7); // jalr t1
    assert_eq!(jalr(ZERO, T1, 0), 0x0003_0067); // jr t1
  }

  #[test]
  fn loads_literals() {
    let addresses = Shim { base: 0, dispatch: 0x1111, handler: 0x2222, trampoline: 0x3333 };
    let code = shim(&addresses);
    let words: Vec<u32> = code.chunks(4).map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap())).collect();
    let literal = |index: usize| u64::from_le_bytes(code[index..index + 8].try_into().unwrap());

    // Emulates each `AUIPC` & `LD` pair
    let mut loaded = Vec::new();
    for (index, pair) in words.windows(2).enumerate() {
      if pair[0] & 0x7F == 0x17 {
        let hi = (pair[0] & 0xFFFF_F000) as i32 as i64;
        let lo = (pair[1] as i32 >> 20) as i64;
        loaded.push(literal((4 * index as i64 + hi + lo) as usize));
      }
    }
    assert_eq!(loaded, [0x2222, 0x1111, 0x3333]);
  }
}
