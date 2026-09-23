//! AArch64 support.
//!
//! The target's first instruction is replaced with a direct branch (`B`,
//! ±128 MiB). Detours further away are reached via a relay: an absolute jump
//! allocated near the target. Since only a single instruction is replaced,
//! even the smallest functions can be detoured, and internal branches into
//! the patched area are practically impossible.
//!
//! Functions beginning with a landing pad (`BTI`, `PACIASP`, `PACIBSP`) are
//! patched after it, so indirect calls remain valid on BTI-guarded pages.

use super::Hook;
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock};

/// The maximum distance of generated code from its target.
///
/// Slightly less than the range of `B`, so any branch between the target's
/// prolog and its generated code is guaranteed to be within reach.
const MAX_DISTANCE: usize = 0x07F0_0000;

/// An upper bound for the size of a single relocated instruction.
const MAX_RELOCATED_LEN: usize = 24;

/// Creates a detour of `target` to `detour`.
///
/// # Safety
///
/// `target` must point to executable code.
pub(crate) unsafe fn build(target: *const (), detour: *const ()) -> Result<Hook> {
  let target = target as usize;
  let detour = detour as usize;

  if target % 4 != 0 || detour % 4 != 0 || memory::readable_len(target, 4) < 4 {
    return Err(Error::InvalidCode);
  }

  // SAFETY: The first instruction has been verified to be readable.
  let first = unsafe { (target as *const u32).read() };

  // Landing pads are kept intact; `PAC*SP` signs the link register, which
  // must be authenticated before the detour is entered.
  let (patch_offset, relay_prefix) = match first {
    encode::BTI | encode::BTI_C | encode::BTI_J | encode::BTI_JC => (4, None),
    encode::PACIASP => (4, Some(encode::AUTIASP)),
    encode::PACIBSP => (4, Some(encode::AUTIBSP)),
    _ => (0, None),
  };
  let patch_address = target + patch_offset;
  if memory::readable_len(target, patch_offset + 4) < patch_offset + 4 {
    return Err(Error::InvalidCode);
  }

  let relay = if relay_prefix.is_some() || encode::b(patch_address, detour).is_none() {
    let mut relay = memory::allocate_near(patch_address, MAX_DISTANCE, 24)?;
    let mut emitter = encode::Emitter::new(relay.address());
    if let Some(prefix) = relay_prefix {
      emitter.push(prefix);
    }
    emitter.jump(detour);

    // SAFETY: The relay has just been allocated, and cannot be executing.
    unsafe { relay.write(&emitter.into_bytes())? };
    Some(relay)
  } else {
    None
  };

  let destination = relay.as_ref().map_or(detour, CodeBlock::address);
  let branch = encode::b(patch_address, destination).ok_or(Error::OutOfMemory)?;

  // Relocate every instruction up to, and including, the patched one
  let count = patch_offset / 4 + 1;
  let mut trampoline =
    memory::allocate_near(target, MAX_DISTANCE, (count + 1) * MAX_RELOCATED_LEN)?;
  let mut emitter = encode::Emitter::new(trampoline.address());
  for index in 0..count {
    let pc = target + index * 4;
    // SAFETY: The instructions have been verified to be readable.
    emitter.relocate(unsafe { (pc as *const u32).read() }, pc);
  }
  emitter.jump(patch_address + 4);

  // SAFETY: The trampoline has just been allocated, and cannot be executing.
  unsafe { trampoline.write(&emitter.into_bytes())? };

  // SAFETY: The instruction has been verified to be readable.
  let original = unsafe { (patch_address as *const [u8; 4]).read() }.to_vec();

  Ok(Hook {
    patch_address: patch_address as *mut u8,
    original,
    patched: branch.to_le_bytes().to_vec(),
    trampoline,
    relay,
  })
}

/// A minimal A64 encoder and relocator.
///
/// This module is platform-independent, so it is unit tested on all hosts.
pub(crate) mod encode {
  use alloc::vec::Vec;

  pub const NOP: u32 = 0xD503_201F;
  pub const BTI: u32 = 0xD503_241F;
  pub const BTI_C: u32 = 0xD503_245F;
  pub const BTI_J: u32 = 0xD503_249F;
  pub const BTI_JC: u32 = 0xD503_24DF;
  pub const PACIASP: u32 = 0xD503_233F;
  pub const PACIBSP: u32 = 0xD503_237F;
  pub const AUTIASP: u32 = 0xD503_23BF;
  pub const AUTIBSP: u32 = 0xD503_23FF;

  /// The intra-procedure-call scratch register (IP0).
  ///
  /// It may be clobbered by linker veneers at any function call, so it never
  /// holds a live value at the start of a function.
  const X16: u32 = 16;

  /// Sign-extends the lower `bits` of `value`.
  fn sign_extend(value: u32, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((u64::from(value) << shift) as i64) >> shift
  }

  /// Returns the immediate of a PC-relative offset, if it fits in `bits`
  /// (in units of instructions).
  fn offset(from: usize, to: usize, bits: u32) -> Option<u32> {
    let delta = (to as i64).wrapping_sub(from as i64);
    let limit = 1i64 << (bits + 1);
    (delta % 4 == 0 && (-limit..limit).contains(&delta))
      .then(|| ((delta >> 2) as u32) & ((1 << bits) - 1))
  }

  /// `B <to>`
  pub fn b(from: usize, to: usize) -> Option<u32> {
    offset(from, to, 26).map(|imm| 0x1400_0000 | imm)
  }

  /// `BL <to>`
  pub fn bl(from: usize, to: usize) -> Option<u32> {
    offset(from, to, 26).map(|imm| 0x9400_0000 | imm)
  }

  /// `LDR Xt, <pc + offset>`
  fn ldr_literal(rt: u32, offset: i32) -> u32 {
    0x5800_0000 | ((((offset >> 2) as u32) & 0x7_FFFF) << 5) | rt
  }

  /// `BR Xn`
  fn br(rn: u32) -> u32 {
    0xD61F_0000 | (rn << 5)
  }

  /// `BLR Xn`
  fn blr(rn: u32) -> u32 {
    0xD63F_0000 | (rn << 5)
  }

  /// Emits A64 code for a known address.
  pub struct Emitter {
    base: usize,
    code: Vec<u32>,
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
      self.base + self.code.len() * 4
    }

    /// Appends an instruction.
    pub fn push(&mut self, instruction: u32) {
      self.code.push(instruction);
    }

    /// Returns the code as bytes.
    pub fn into_bytes(self) -> Vec<u8> {
      self.code.into_iter().flat_map(u32::to_le_bytes).collect()
    }

    /// Returns the emitted instructions.
    #[cfg(test)]
    pub fn instructions(&self) -> &[u32] {
      &self.code
    }

    /// Appends a 64-bit literal.
    fn literal(&mut self, value: u64) {
      self.push(value as u32);
      self.push((value >> 32) as u32);
    }

    /// Loads a 64-bit value into `rt`.
    fn load(&mut self, rt: u32, value: u64) {
      // LDR Xt, #8; B #12; .quad value
      self.push(ldr_literal(rt, 8));
      self.push(0x1400_0003);
      self.literal(value);
    }

    /// Branches to `destination`.
    ///
    /// A direct branch is preferred, since indirect branches must target
    /// landing pads on BTI-guarded pages.
    pub fn jump(&mut self, destination: usize) {
      if let Some(branch) = b(self.pc(), destination) {
        self.push(branch);
      } else {
        // LDR X16, #8; BR X16; .quad destination
        self.push(ldr_literal(X16, 8));
        self.push(br(X16));
        self.literal(destination as u64);
      }
    }

    /// Calls `destination`, i.e. branches with link.
    fn call(&mut self, destination: usize) {
      if let Some(branch) = bl(self.pc(), destination) {
        self.push(branch);
      } else {
        self.load(X16, destination as u64);
        self.push(blr(X16));
      }
    }

    /// Emits `condition` (a branch with an inverted condition, whose offset is
    /// yet to be determined), followed by a jump to `destination`.
    fn conditional_jump(&mut self, destination: usize, skip: impl FnOnce(i64) -> u32) {
      let index = self.code.len();
      self.push(NOP);
      self.jump(destination);
      let distance = ((self.code.len() - index) * 4) as i64;
      self.code[index] = skip(distance);
    }

    /// Relocates `instruction`, originally located at `pc`.
    pub fn relocate(&mut self, instruction: u32, pc: usize) {
      let target = |imm: u32, bits: u32| pc.wrapping_add((sign_extend(imm, bits) * 4) as usize);

      match instruction {
        // B <label>
        i if i & 0xFC00_0000 == 0x1400_0000 => self.jump(target(i & 0x3FF_FFFF, 26)),
        // BL <label>
        i if i & 0xFC00_0000 == 0x9400_0000 => self.call(target(i & 0x3FF_FFFF, 26)),
        // B.cond <label> & BC.cond <label>
        i if i & 0xFF00_0000 == 0x5400_0000 => {
          let condition = i & 0xF;
          let destination = target((i >> 5) & 0x7_FFFF, 19);
          if condition >= 0b1110 {
            // AL & NV are unconditional
            self.jump(destination);
          } else {
            let inverted = (i & 0xFF00_0010) | (condition ^ 1);
            self.conditional_jump(destination, |skip| inverted | (((skip >> 2) as u32) << 5));
          }
        },
        // CBZ/CBNZ <Rt>, <label>
        i if i & 0x7E00_0000 == 0x3400_0000 => {
          let destination = target((i >> 5) & 0x7_FFFF, 19);
          let inverted = (i ^ (1 << 24)) & !(0x7_FFFF << 5);
          self.conditional_jump(destination, |skip| inverted | (((skip >> 2) as u32) << 5));
        },
        // TBZ/TBNZ <Rt>, #<imm>, <label>
        i if i & 0x7E00_0000 == 0x3600_0000 => {
          let destination = target((i >> 5) & 0x3FFF, 14);
          let inverted = (i ^ (1 << 24)) & !(0x3FFF << 5);
          self.conditional_jump(destination, |skip| inverted | (((skip >> 2) as u32) << 5));
        },
        // ADR <Xd>, <label>
        i if i & 0x9F00_0000 == 0x1000_0000 => {
          let imm = ((i >> 5) & 0x7_FFFF) << 2 | ((i >> 29) & 0x3);
          let value = pc.wrapping_add(sign_extend(imm, 21) as usize);
          self.load(i & 0x1F, value as u64);
        },
        // ADRP <Xd>, <label>
        i if i & 0x9F00_0000 == 0x9000_0000 => {
          let imm = ((i >> 5) & 0x7_FFFF) << 2 | ((i >> 29) & 0x3);
          let value = (pc & !0xFFF).wrapping_add((sign_extend(imm, 21) << 12) as usize);
          self.load(i & 0x1F, value as u64);
        },
        // LDR/LDRSW/PRFM (literal), including SIMD & FP registers
        i if i & 0x3B00_0000 == 0x1800_0000 => {
          let address = target((i >> 5) & 0x7_FFFF, 19) as u64;
          let rt = i & 0x1F;
          let is_simd = i & (1 << 26) != 0;

          match (i >> 30, is_simd) {
            // PRFM is merely a hint; an unencodable one is a NOP
            (0b11, false) => self.push(NOP),
            // Loads to the zero register have no effect
            (_, false) if rt == 31 => self.push(NOP),
            (opc, false) => {
              self.load(rt, address);
              self.push(
                match opc {
                  0b00 => 0xB940_0000, // LDR Wt, [Xt]
                  0b01 => 0xF940_0000, // LDR Xt, [Xt]
                  _ => 0xB980_0000,    // LDRSW Xt, [Xt]
                } | (rt << 5)
                  | rt,
              );
            },
            (opc, true) => {
              self.load(X16, address);
              self.push(
                match opc {
                  0b00 => 0xBD40_0000, // LDR St, [X16]
                  0b01 => 0xFD40_0000, // LDR Dt, [X16]
                  _ => 0x3DC0_0000,    // LDR Qt, [X16]
                } | (X16 << 5)
                  | rt,
              );
            },
          }
        },
        // Position-independent
        i => self.push(i),
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

  fn relocate(instruction: u32, base: usize) -> Vec<u32> {
    let mut emitter = Emitter::new(base);
    emitter.relocate(instruction, PC);
    emitter.instructions().to_vec()
  }

  #[test]
  fn branch_encoding() {
    assert_eq!(b(PC, PC + 8), Some(0x1400_0002));
    assert_eq!(b(PC, PC - 4), Some(0x17FF_FFFF));
    assert_eq!(bl(PC, PC + 0x400), Some(0x9400_0100));
    assert_eq!(b(PC, PC + 0x800_0000), None);
    assert_eq!(b(PC, PC - 0x800_0000), Some(0x1600_0000));
    assert_eq!(b(PC, PC + 2), None);
  }

  #[test]
  fn position_independent_instructions_are_copied() {
    // add x0, x1, x2; ret; paciasp; ldr x0, [x1]
    for instruction in [0x8B02_0020, 0xD65F_03C0, PACIASP, 0xF940_0020] {
      assert_eq!(relocate(instruction, FAR), [instruction]);
    }
  }

  #[test]
  fn relocates_branch() {
    // b #+0x100
    assert_eq!(
      relocate(0x1400_0040, PC + 0x1000),
      [b(PC + 0x1000, PC + 0x100).unwrap()]
    );
    assert_eq!(
      relocate(0x1400_0040, FAR),
      [0x5800_0050, 0xD61F_0200, 0x0000_0100, 0x1]
    );
  }

  #[test]
  fn relocates_branch_with_link() {
    // bl #-0x10
    assert_eq!(
      relocate(0x97FF_FFFC, PC + 0x10),
      [bl(PC + 0x10, PC - 0x10).unwrap()]
    );
    assert_eq!(
      relocate(0x97FF_FFFC, FAR),
      [0x5800_0050, 0x1400_0003, 0xFFFF_FFF0, 0x0, 0xD63F_0200]
    );
  }

  #[test]
  fn relocates_conditional_branch() {
    // b.eq #+0x20 → b.ne #+8; b <dest>
    assert_eq!(
      relocate(0x5400_0100, PC + 0x100),
      [0x5400_0041, b(PC + 0x104, PC + 0x20).unwrap()]
    );
    // b.ne #+0x20 → b.eq #+20; ldr x16, #8; br x16; .quad dest
    assert_eq!(
      relocate(0x5400_0101, FAR),
      [0x5400_00A0, 0x5800_0050, 0xD61F_0200, 0x0000_0020, 0x1]
    );
    // b.al is unconditional
    assert_eq!(
      relocate(0x5400_010E, PC + 0x100),
      [b(PC + 0x100, PC + 0x20).unwrap()]
    );
  }

  #[test]
  fn relocates_compare_and_branch() {
    // cbz x3, #+0x40 → cbnz x3, #+8; b <dest>
    assert_eq!(
      relocate(0xB400_0203, PC + 0x100),
      [0xB500_0043, b(PC + 0x104, PC + 0x40).unwrap()]
    );
    // cbnz w5, #-0x8 → cbz w5, #+20; ...
    assert_eq!(relocate(0x35FF_FFC5, FAR)[0], 0x3400_00A5);
  }

  #[test]
  fn relocates_test_and_branch() {
    // tbz w1, #3, #+0x10 → tbnz w1, #3, #+8; b <dest>
    assert_eq!(
      relocate(0x3618_0081, PC + 0x100),
      [0x3718_0041, b(PC + 0x104, PC + 0x10).unwrap()]
    );
  }

  #[test]
  fn relocates_address_calculations() {
    // adr x2, #+0x11
    assert_eq!(
      relocate(0x3000_0082, FAR),
      [0x5800_0042, 0x1400_0003, 0x0000_0011, 0x1]
    );
    // adrp x0, #+0x2000 (from a non-page-aligned pc)
    let mut emitter = Emitter::new(FAR);
    emitter.relocate(0xD000_0000, PC + 0x123);
    assert_eq!(
      emitter.instructions(),
      [0x5800_0040, 0x1400_0003, 0x0000_2000, 0x1]
    );
  }

  #[test]
  fn relocates_literal_loads() {
    // ldr w1, #+0x8
    assert_eq!(
      relocate(0x1800_0041, FAR),
      [0x5800_0041, 0x1400_0003, 0x0000_0008, 0x1, 0xB940_0021]
    );
    // ldr x1, #+0x8
    assert_eq!(relocate(0x5800_0041, FAR)[4], 0xF940_0021);
    // ldrsw x1, #+0x8
    assert_eq!(relocate(0x9800_0041, FAR)[4], 0xB980_0021);
    // ldr q7, #+0x8
    assert_eq!(
      relocate(0x9C00_0047, FAR),
      [0x5800_0050, 0x1400_0003, 0x0000_0008, 0x1, 0x3DC0_0207]
    );
    // prfm pldl1keep, #+0x8
    assert_eq!(relocate(0xD800_0040, FAR), [NOP]);
  }
}
