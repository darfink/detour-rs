//! RISC-V (RV64) support.
//!
//! The target is redirected using `AUIPC T1, hi; JALR X0, lo(T1)` (8 bytes,
//! ±2 GiB). Detours further away are reached via a relay, an absolute jump
//! allocated near the target.
//!
//! T1 is clobbered, since it holds no value upon function entry: the
//! dynamic linker's PLT stubs clobber it as well. Instructions that are
//! relocated must not use it.
//!
//! With the compressed extension, instructions are 2 or 4 bytes long, so the
//! patch may span several instructions. It is not atomic, and the target must
//! not be executing whilst the detour is toggled (see `Transaction`). The
//! function is scanned for branches into the patched instructions, which are
//! rejected.

use super::Redirect;
#[cfg(target_arch = "riscv64")]
use super::Site;
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock};
#[cfg(target_arch = "riscv64")]
use alloc::vec::Vec;

/// The maximum distance of generated code from its target.
///
/// Slightly less than 2 GiB, so any branch between the target's prolog and
/// its generated code is within reach of `AUIPC` & `JALR`.
const MAX_DISTANCE: usize = 0x7FF0_0000;

/// `AUIPC` & `JALR`
const PATCH_LEN: usize = 8;

/// An upper bound for the size of a single relocated instruction.
const MAX_RELOCATED_LEN: usize = 32;

/// How a target is patched, which is the same for all targets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout;

/// Prepares the site of `target`, and redirects it to `detour`.
///
/// # Safety
///
/// `target` must point to executable code.
#[cfg(target_arch = "riscv64")]
pub(crate) unsafe fn build(target: *const (), detour: *const ()) -> Result<(Site, Redirect)> {
  let target = target as usize;
  if target % 2 != 0 || detour as usize % 2 != 0 {
    return Err(Error::InvalidInstruction);
  }

  // The function is inspected beyond the patched instructions, for branches
  // into them
  const SCAN_LEN: usize = 1024;
  let available = memory::readable_len(target, SCAN_LEN);
  // SAFETY: The range has been verified to be readable.
  let code = unsafe { core::slice::from_raw_parts(target as *const u8, available) };

  // Decode the instructions spanned by the patch
  let mut prolog = Vec::new();
  let mut len = 0;
  while len < PATCH_LEN {
    let (instruction, size) = decode(code, len).ok_or(Error::InvalidInstruction)?;
    prolog.push((instruction, len));
    len += size;

    if encode::is_terminator(instruction) && len < PATCH_LEN {
      // The rest of the patch must be padding following the function
      if !is_padding(code, len..PATCH_LEN) {
        return Err(Error::PatchAreaTooSmall);
      }
      break;
    }
  }
  let terminated = prolog.last().is_some_and(|&(instruction, _)| encode::is_terminator(instruction));

  // The relocated code clobbers T1 (e.g. to reach the original function)
  if prolog.iter().any(|&(instruction, _)| encode::may_use_t1(instruction)) {
    return Err(Error::UnsupportedInstruction);
  }

  let patched = (target + 1)..(target + PATCH_LEN);
  let mut offset = 0;
  while let Some((instruction, size)) = decode(code, offset) {
    let destination = encode::branch_target(instruction, target + offset);
    if destination.is_some_and(|destination| patched.contains(&destination)) {
      return Err(Error::UnsupportedInstruction);
    }
    offset += size;
  }

  let mut trampoline =
    memory::allocate_near(target, MAX_DISTANCE, (prolog.len() + 1) * MAX_RELOCATED_LEN)?;
  let mut emitter = encode::Emitter::new(trampoline.address());
  let mut relocated = Vec::with_capacity(prolog.len());
  for &(instruction, offset) in &prolog {
    relocated.push((offset as u32, emitter.offset()));
    emitter.relocate(instruction, target + offset);
  }
  if !terminated {
    emitter.jump(target + len);
  }

  // SAFETY: The trampoline has just been allocated, and cannot be executing.
  unsafe { trampoline.write(&emitter.into_bytes())? };

  let redirect = redirect(target as *mut u8, Layout, detour)?;
  // SAFETY: The patch area has been verified to be readable.
  let original = unsafe { core::slice::from_raw_parts(target as *const u8, PATCH_LEN) }.to_vec();

  let site = Site {
    patch_address: target as *mut u8,
    original,
    trampoline,
    relocated,
    layout: Layout,
  };
  Ok((site, redirect))
}

/// Redirects the site patched at `patch_address` to `detour`.
pub(crate) fn redirect(patch_address: *mut u8, _: Layout, detour: *const ()) -> Result<Redirect> {
  let (patch_address, detour) = (patch_address as usize, detour as usize);

  let relay = if encode::split(patch_address, detour).is_some() {
    None
  } else {
    let mut relay = memory::allocate_near(patch_address, MAX_DISTANCE, MAX_RELOCATED_LEN)?;
    let mut emitter = encode::Emitter::new(relay.address());
    emitter.absolute_jump(detour);
    // SAFETY: The relay has just been allocated, and cannot be executing.
    unsafe { relay.write(&emitter.into_bytes())? };
    Some(relay)
  };

  let destination = relay.as_ref().map_or(detour, CodeBlock::address);
  let (hi, lo) = encode::split(patch_address, destination).ok_or(Error::NoNearbyMemory)?;
  let patched = [encode::auipc(encode::T1, hi), encode::jalr(encode::ZERO, encode::T1, lo)];
  Ok(Redirect {
    patched: patched.iter().flat_map(|instruction| instruction.to_le_bytes()).collect(),
    relay,
  })
}

/// Returns a stub jumping to the address stored at `pointer`, allocated near
/// `origin`.
///
/// It clobbers T1, the scratch register (see [`RawDetour::new`]).
///
/// # Safety
///
/// The caller must hold the global patch lock.
///
/// [`RawDetour::new`]: crate::RawDetour::new
pub(crate) unsafe fn link(pointer: *const usize, origin: usize) -> Result<CodeBlock> {
  let mut emitter = encode::Emitter::new(0);
  emitter.indirect_jump(pointer as u64);
  let code = emitter.into_bytes();
  let mut stub = memory::allocate_near(origin, usize::MAX, code.len())?;
  // SAFETY: The stub has just been allocated, and cannot be executing.
  unsafe { stub.write(&code)? };
  Ok(stub)
}

/// Decodes the instruction at `offset` of `code`, returning it and its
/// length, or `None` if it is truncated or of an unsupported length.
fn decode(code: &[u8], offset: usize) -> Option<(u32, usize)> {
  let halfword = |offset: usize| Some(u16::from_le_bytes(code.get(offset..offset + 2)?.try_into().ok()?));
  let low = halfword(offset)?;
  match encode::length(low)? {
    2 => Some((u32::from(low), 2)),
    _ => Some((u32::from(low) | u32::from(halfword(offset + 2)?) << 16, 4)),
  }
}

/// Returns whether `range` of `code` consists of padding only (`NOP`, `C.NOP`
/// or zeroes, i.e. `C.UNIMP`).
fn is_padding(code: &[u8], range: core::ops::Range<usize>) -> bool {
  let mut offset = range.start;
  while offset < range.end {
    match decode(code, offset) {
      Some((encode::NOP, size) | (encode::C_NOP | 0, size)) => offset += size,
      _ => return false,
    }
  }
  true
}

/// A minimal RV64 encoder and relocator.
///
/// This module is platform-independent, so it is unit tested on all hosts.
pub(crate) mod encode {
  use alloc::vec::Vec;

  pub const ZERO: u32 = 0;
  /// The scratch register (X6).
  pub const T1: u32 = 6;

  /// `ADDI X0, X0, 0`
  pub const NOP: u32 = 0x0000_0013;
  /// `C.ADDI X0, 0`
  pub const C_NOP: u32 = 0x0001;

  /// Returns the length of an instruction from its lowest halfword, if it is
  /// 2 or 4 bytes long.
  pub fn length(low: u16) -> Option<usize> {
    match low & 0b11 {
      0b11 if low & 0b1_1100 != 0b1_1100 => Some(4),
      0b11 => None,
      _ => Some(2),
    }
  }

  /// Returns whether `instruction` is compressed (2 bytes long).
  pub fn is_compressed(instruction: u32) -> bool {
    instruction & 0b11 != 0b11
  }

  /// Sign-extends the lower `bits` of `value`.
  fn sign_extend(value: u32, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((u64::from(value) << shift) as i64) >> shift
  }

  fn bit(value: u32, index: u32) -> u32 {
    (value >> index) & 1
  }

  fn bits(value: u32, high: u32, low: u32) -> u32 {
    (value >> low) & ((1 << (high - low + 1)) - 1)
  }

  /// A decoded PC-relative instruction.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  enum Kind {
    /// `AUIPC rd, imm`, with the value it computes relative to the PC.
    Auipc { rd: u32, offset: i64 },
    /// `JAL rd, offset` (`C.J` with `rd = X0`).
    Jal { rd: u32, offset: i64 },
    /// A conditional branch (`C.BEQZ`/`C.BNEZ` with `rs2 = X0`).
    Branch { funct3: u32, rs1: u32, rs2: u32, offset: i64 },
  }

  fn classify(instruction: u32) -> Option<Kind> {
    if is_compressed(instruction) {
      let funct3 = bits(instruction, 15, 13);
      return match (instruction & 0b11, funct3) {
        // C.J
        (0b01, 0b101) => {
          let imm = bit(instruction, 12) << 11
            | bit(instruction, 8) << 10
            | bits(instruction, 10, 9) << 8
            | bit(instruction, 6) << 7
            | bit(instruction, 7) << 6
            | bit(instruction, 2) << 5
            | bit(instruction, 11) << 4
            | bits(instruction, 5, 3) << 1;
          Some(Kind::Jal { rd: ZERO, offset: sign_extend(imm, 12) })
        },
        // C.BEQZ & C.BNEZ
        (0b01, 0b110 | 0b111) => {
          let imm = bit(instruction, 12) << 8
            | bits(instruction, 6, 5) << 6
            | bit(instruction, 2) << 5
            | bits(instruction, 11, 10) << 3
            | bits(instruction, 4, 3) << 1;
          Some(Kind::Branch {
            funct3: funct3 & 1,
            rs1: 8 + bits(instruction, 9, 7),
            rs2: ZERO,
            offset: sign_extend(imm, 9),
          })
        },
        _ => None,
      };
    }

    let rd = bits(instruction, 11, 7);
    match instruction & 0x7F {
      // AUIPC; with `rd = X0` it is a hint (e.g. `LPAD`), without effect
      0x17 if rd != ZERO => Some(Kind::Auipc {
        rd,
        offset: sign_extend(instruction & 0xFFFF_F000, 32),
      }),
      // JAL
      0x6F => {
        let imm = bit(instruction, 31) << 20
          | bits(instruction, 19, 12) << 12
          | bit(instruction, 20) << 11
          | bits(instruction, 30, 21) << 1;
        Some(Kind::Jal { rd, offset: sign_extend(imm, 21) })
      },
      // BEQ, BNE, BLT, BGE, BLTU & BGEU
      0x63 if !matches!(bits(instruction, 14, 12), 0b010 | 0b011) => {
        let imm = bit(instruction, 31) << 12
          | bit(instruction, 7) << 11
          | bits(instruction, 30, 25) << 5
          | bits(instruction, 11, 8) << 1;
        Some(Kind::Branch {
          funct3: bits(instruction, 14, 12),
          rs1: bits(instruction, 19, 15),
          rs2: bits(instruction, 24, 20),
          offset: sign_extend(imm, 13),
        })
      },
      _ => None,
    }
  }

  /// Returns whether `instruction` unconditionally leaves the function (e.g.
  /// `J`, `JR` & `RET`, but not calls).
  pub fn is_terminator(instruction: u32) -> bool {
    if is_compressed(instruction) {
      // C.J, or C.JR (`rs2 = X0`)
      let is_jump = instruction & 0xE003 == 0xA001;
      let is_jump_register = instruction & 0xF07F == 0x8002 && bits(instruction, 11, 7) != 0;
      return is_jump || is_jump_register;
    }
    // JAL & JALR with `rd = X0`
    matches!(instruction & 0x7F, 0x6F | 0x67) && bits(instruction, 11, 7) == ZERO
  }

  /// Returns whether `instruction` may reference T1, by inspecting the
  /// register fields of its format (conservatively, every field of an unknown
  /// format, and floating-point registers as well).
  pub fn may_use_t1(instruction: u32) -> bool {
    const RD: u32 = 7;
    const RS1: u32 = 15;
    const RS2: u32 = 20;
    const RS3: u32 = 27;

    let fields: &[u32] = if is_compressed(instruction) {
      // 3-bit register fields (X8–X15) cannot encode X6
      match (instruction & 0b11, bits(instruction, 15, 13)) {
        (0b00, _) | (0b01, 0b100..=0b111) => &[],
        // C.ADDI, C.LI, C.ADDIW, C.LUI & C.ADDI16SP
        (0b01, _) => &[7],
        // C.JR, C.MV, C.JALR & C.ADD
        (0b10, 0b100) => &[7, 2],
        // Stores relative to SP
        (0b10, 0b101..=0b111) => &[2],
        // C.SLLI & loads relative to SP
        _ => &[7],
      }
    } else {
      match instruction & 0x7F {
        // LUI, AUIPC & JAL
        0x37 | 0x17 | 0x6F => &[RD],
        // Immediates, loads, JALR, fences & system instructions
        0x13 | 0x1B | 0x03 | 0x07 | 0x67 | 0x0F | 0x73 => &[RD, RS1],
        // Stores & branches
        0x23 | 0x27 | 0x63 => &[RS1, RS2],
        // Register operations & atomics
        0x33 | 0x3B | 0x2F => &[RD, RS1, RS2],
        _ => &[RD, RS1, RS2, RS3],
      }
    };
    fields.iter().any(|&shift| (instruction >> shift) & 0x1F == T1)
  }

  /// Returns the destination of a PC-relative branch at `pc`, if any.
  pub fn branch_target(instruction: u32, pc: usize) -> Option<usize> {
    match classify(instruction)? {
      Kind::Jal { offset, .. } | Kind::Branch { offset, .. } => Some(pc.wrapping_add(offset as usize)),
      Kind::Auipc { .. } => None,
    }
  }

  /// Returns the upper and lower immediates of `AUIPC` & a 12-bit
  /// instruction at `from`, reaching `to`, if within ±2 GiB.
  pub fn split(from: usize, to: usize) -> Option<(u32, u32)> {
    let delta = (to as i64).wrapping_sub(from as i64);
    let hi = (delta + 0x800) >> 12;
    let lo = delta - (hi << 12);
    (-(1 << 19)..(1 << 19)).contains(&hi).then_some(((hi as u32) & 0xF_FFFF, (lo as u32) & 0xFFF))
  }

  /// `AUIPC rd, hi`
  pub fn auipc(rd: u32, hi: u32) -> u32 {
    hi << 12 | rd << 7 | 0x17
  }

  /// `ADDI rd, rs1, lo`
  fn addi(rd: u32, rs1: u32, lo: u32) -> u32 {
    lo << 20 | rs1 << 15 | rd << 7 | 0x13
  }

  /// `LD rd, lo(rs1)`
  fn ld(rd: u32, rs1: u32, lo: u32) -> u32 {
    lo << 20 | rs1 << 15 | 0b011 << 12 | rd << 7 | 0x03
  }

  /// `JALR rd, lo(rs1)`
  pub fn jalr(rd: u32, rs1: u32, lo: u32) -> u32 {
    lo << 20 | rs1 << 15 | rd << 7 | 0x67
  }

  /// `JAL rd, <to>`
  pub fn jal(rd: u32, from: usize, to: usize) -> Option<u32> {
    let offset = (to as i64).wrapping_sub(from as i64);
    ((-(1 << 20)..(1 << 20)).contains(&offset) && offset % 2 == 0).then(|| {
      let imm = offset as u32;
      bit(imm, 20) << 31 | bits(imm, 10, 1) << 21 | bit(imm, 11) << 20 | bits(imm, 19, 12) << 12 | rd << 7 | 0x6F
    })
  }

  /// `B<cond> rs1, rs2, <pc + offset>`
  fn branch(funct3: u32, rs1: u32, rs2: u32, offset: i64) -> u32 {
    let imm = offset as u32;
    bit(imm, 12) << 31
      | bits(imm, 10, 5) << 25
      | rs2 << 20
      | rs1 << 15
      | funct3 << 12
      | bits(imm, 4, 1) << 8
      | bit(imm, 11) << 7
      | 0x63
  }

  /// Emits RV64 code for a known address.
  pub struct Emitter {
    base: usize,
    code: Vec<u8>,
  }

  impl Emitter {
    /// Creates an emitter for code placed at `base`.
    pub fn new(base: usize) -> Self {
      Emitter {
        base,
        code: Vec::new(),
      }
    }

    /// Returns the address of the next instruction.
    fn pc(&self) -> usize {
      self.base + self.code.len()
    }

    /// Returns the offset of the next instruction, in bytes.
    pub fn offset(&self) -> u32 {
      self.code.len() as u32
    }

    /// Appends a 4-byte instruction.
    fn push(&mut self, instruction: u32) {
      self.code.extend_from_slice(&instruction.to_le_bytes());
    }

    /// Appends a 2-byte instruction.
    fn push_compressed(&mut self, instruction: u32) {
      self.code.extend_from_slice(&(instruction as u16).to_le_bytes());
    }

    /// Returns the code as bytes.
    pub fn into_bytes(self) -> Vec<u8> {
      self.code
    }

    /// Returns the emitted code.
    #[cfg(test)]
    pub fn bytes(&self) -> &[u8] {
      &self.code
    }

    /// Loads a 64-bit literal into `rd`, followed by `suffix` and a jump past
    /// the literal.
    fn literal(&mut self, rd: u32, value: u64, suffix: &[u32]) {
      // AUIPC rd, 0; LD rd, off(rd); <suffix>; J <end>; <padding>; .dword value
      let start = self.pc();
      let literal = (start + 12 + suffix.len() * 4).next_multiple_of(8);
      self.push(auipc(rd, 0));
      self.push(ld(rd, rd, (literal - start) as u32));
      for &instruction in suffix {
        self.push(instruction);
      }
      let end = literal + 8;
      self.push(jal(ZERO, self.pc(), end).expect("a nearby literal"));
      self.code.resize(literal - self.base, 0);
      self.code.extend_from_slice(&value.to_le_bytes());
    }

    /// Branches to `destination`, clobbering T1 if it is out of reach of
    /// `JAL`.
    pub fn jump(&mut self, destination: usize) {
      self.call(ZERO, destination);
    }

    /// Branches to `destination` using an absolute address, clobbering T1.
    pub fn absolute_jump(&mut self, destination: usize) {
      self.literal(T1, destination as u64, &[jalr(ZERO, T1, 0)]);
    }

    /// Branches to the address stored at `pointer`, clobbering T1.
    pub fn indirect_jump(&mut self, pointer: u64) {
      self.literal(T1, pointer, &[ld(T1, T1, 0), jalr(ZERO, T1, 0)]);
    }

    /// Branches to `destination`, linking `rd` (which is used as scratch, or
    /// T1 if it is X0).
    fn call(&mut self, rd: u32, destination: usize) {
      let scratch = if rd == ZERO { T1 } else { rd };
      if let Some(instruction) = jal(rd, self.pc(), destination) {
        self.push(instruction);
      } else if let Some((hi, lo)) = split(self.pc(), destination) {
        self.push(auipc(scratch, hi));
        self.push(jalr(rd, scratch, lo));
      } else {
        // The return address is the jump past the literal
        self.literal(scratch, destination as u64, &[jalr(rd, scratch, 0)]);
      }
    }

    /// Relocates `instruction` (2 or 4 bytes), originally located at `pc`.
    pub fn relocate(&mut self, instruction: u32, pc: usize) {
      let target = |offset: i64| pc.wrapping_add(offset as usize);

      match classify(instruction) {
        Some(Kind::Auipc { rd, offset }) => {
          let value = target(offset);
          if let Some((hi, lo)) = split(self.pc(), value) {
            self.push(auipc(rd, hi));
            self.push(addi(rd, rd, lo));
          } else {
            self.literal(rd, value as u64, &[]);
          }
        },
        Some(Kind::Jal { rd, offset }) => self.call(rd, target(offset)),
        Some(Kind::Branch { funct3, rs1, rs2, offset }) => {
          // An inverted branch, skipping a jump to the destination
          let index = self.code.len();
          self.push(NOP);
          self.jump(target(offset));
          let skip = (self.code.len() - index) as i64;
          let inverted = branch(funct3 ^ 1, rs1, rs2, skip);
          self.code[index..index + 4].copy_from_slice(&inverted.to_le_bytes());
        },
        None if is_compressed(instruction) => self.push_compressed(instruction),
        None => self.push(instruction),
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::encode::*;
  use alloc::vec::Vec;

  const PC: usize = 0x1_0000_0000;
  const FAR: usize = 0x7_0000_0000;

  fn relocate(instruction: u32, base: usize) -> Vec<u8> {
    let mut emitter = Emitter::new(base);
    emitter.relocate(instruction, PC);
    emitter.bytes().to_vec()
  }

  fn words(code: &[u8]) -> Vec<u32> {
    code.chunks(4).map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap())).collect()
  }

  #[test]
  fn decodes_lengths() {
    assert_eq!(length(0x0001), Some(2)); // c.nop
    assert_eq!(length(0x8082), Some(2)); // ret (c.jr ra)
    assert_eq!(length(0x0013), Some(4)); // nop
    assert_eq!(length(0x001F), None); // 48-bit
  }

  #[test]
  fn classifies_terminators() {
    // ret; c.j; jal x0; jr t0 (jalr x0, 0(t0))
    for instruction in [0x8082, 0xA001, 0x0000_006F, 0x0002_8067] {
      assert!(is_terminator(instruction), "{instruction:#x}");
    }
    // jalr ra (call); c.jalr t0; jal ra; c.mv a0, a1; nop
    for instruction in [0x0000_80E7, 0x9282, 0x0000_00EF, 0x852E, NOP] {
      assert!(!is_terminator(instruction), "{instruction:#x}");
    }
  }

  #[test]
  fn resolves_branch_targets() {
    assert_eq!(branch_target(0x0100_006F, PC), Some(PC + 0x10)); // j +0x10
    assert_eq!(branch_target(0xFFDF_F0EF, PC), Some(PC - 4)); // jal ra, -4
    assert_eq!(branch_target(0x00B5_0463, PC), Some(PC + 8)); // beq a0, a1, +8
    assert_eq!(branch_target(0xFE05_4EE3, PC), Some(PC - 4)); // blt a0, x0, -4
    assert_eq!(branch_target(0xA021, PC), Some(PC + 8)); // c.j +8
    assert_eq!(branch_target(0xC111, PC), Some(PC + 4)); // c.beqz a0, +4
    assert_eq!(branch_target(0xE111, PC), Some(PC + 4)); // c.bnez a0, +4
    assert_eq!(branch_target(0x8082, PC), None); // ret
    assert_eq!(branch_target(0x0000_0517, PC), None); // auipc a0, 0
  }

  #[test]
  fn detects_t1_usage() {
    assert!(may_use_t1(0x0010_0313)); // li t1, 1
    assert!(may_use_t1(0x0003_0513)); // mv a0, t1
    assert!(may_use_t1(0x851A)); // c.mv a0, t1
    assert!(may_use_t1(0x0063_0023)); // sb t1, 0(t1)
    assert!(!may_use_t1(0x00B5_0533)); // add a0, a0, a1
    // Immediates are not registers
    assert!(!may_use_t1(0x0060_0513)); // li a0, 6
    assert!(!may_use_t1(0x4519)); // c.li a0, 6
    assert!(!may_use_t1(0x0003_0537)); // lui a0, 0x30
  }

  #[test]
  fn encodes_jumps() {
    assert_eq!(jal(ZERO, PC, PC + 0x10), Some(0x0100_006F));
    assert_eq!(jal(1, PC, PC - 4), Some(0xFFDF_F0EF));
    assert_eq!(jal(ZERO, PC, PC + 0x10_0000), None);
    assert_eq!(split(PC, PC + 0x1234), Some((0x1, 0x234)));
    assert_eq!(split(PC, PC + 0x1800), Some((0x2, 0x800)));
    assert_eq!(split(PC, PC + 0x8000_0000), None);
  }

  #[test]
  fn position_independent_instructions_are_copied() {
    // nop; add a0, a0, a1; lpad (auipc x0); c.nop; ret
    for instruction in [NOP, 0x00B5_0533, 0x0000_1017] {
      assert_eq!(words(&relocate(instruction, FAR)), [instruction]);
    }
    for instruction in [C_NOP, 0x8082] {
      assert_eq!(relocate(instruction, FAR), (instruction as u16).to_le_bytes());
    }
  }

  #[test]
  fn relocates_jumps() {
    // j +0x10 → j (within reach), or auipc t1; jalr x0 (beyond it)
    assert_eq!(words(&relocate(0x0100_006F, PC + 0x100)), [jal(ZERO, PC + 0x100, PC + 0x10).unwrap()]);
    assert_eq!(words(&relocate(0x0100_006F, PC + 0x20_0000)), [0xFFE0_0317, 0x0103_0067]);
    // jal ra, +0x10 uses its link register as scratch
    assert_eq!(words(&relocate(0x0100_00EF, PC + 0x20_0000)), [0xFFE0_0097, 0x0100_80E7]);
  }

  #[test]
  fn relocates_distant_jumps() {
    // auipc t1, 0; ld t1, 16(t1); jalr x0, 0(t1); j <end>; .dword <dest>
    assert_eq!(
      words(&relocate(0x0100_006F, FAR)),
      [0x0000_0317, 0x0103_3303, 0x0003_0067, 0x00C0_006F, 0x0000_0010, 0x1]
    );
  }

  #[test]
  fn relocates_branches() {
    // beq a0, a1, +0x20 → bne a0, a1, +8; j <dest>
    assert_eq!(
      words(&relocate(0x02B5_0063, PC + 0x100)),
      [0x00B5_1463, jal(ZERO, PC + 0x104, PC + 0x20).unwrap()]
    );
    // c.bnez a0, +4 → beq a0, x0, +12; auipc t1; jalr x0
    assert_eq!(words(&relocate(0xE111, PC + 0x20_0000))[0], 0x0005_0663);
  }

  #[test]
  fn relocates_compressed_jumps() {
    // c.j +8
    assert_eq!(words(&relocate(0xA021, PC + 0x100)), [jal(ZERO, PC + 0x100, PC + 8).unwrap()]);
  }

  #[test]
  fn relocates_address_calculations() {
    // auipc a0, 0x1 → auipc a0, hi; addi a0, a0, lo (relative to the new PC)
    assert_eq!(words(&relocate(0x0000_1517, PC + 0x10)), [0x0000_1517, 0xFF05_0513]);
    // auipc a0, 0x1 → auipc a0, 0; ld a0, 16(a0); j <end>; .dword <value>
    assert_eq!(
      words(&relocate(0x0000_1517, FAR)),
      [0x0000_0517, 0x0105_3503, 0x0100_006F, 0x0, 0x0000_1000, 0x1]
    );
  }
}

#[cfg(test)]
#[path = "riscv64_oracle.rs"]
mod oracle;

#[cfg(all(test, target_arch = "riscv64", target_os = "linux"))]
#[path = "riscv64_differential.rs"]
mod differential;
