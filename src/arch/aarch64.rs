//! AArch64 support.
//!
//! The target's first instruction is replaced with a direct branch (`B`,
//! ±128 MiB). Detours further away are reached via a relay: an absolute jump
//! allocated near the target. Since only a single instruction is replaced,
//! even the smallest functions can be detoured, and internal branches into
//! the patched area are practically impossible.
//!
//! If no memory is available within range of the target (e.g. within the
//! densely packed dyld shared cache on macOS), the target is instead patched
//! with an absolute jump (`LDR X16, #8; BR X16; .quad detour`), replacing four
//! instructions. This requires that no other code branches into them. Unlike the
//! single branch, replacing multiple instructions is not atomic, so the target
//! must not be executing whilst the detour is toggled.
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

#[cfg(test)]
std::thread_local! {
  /// Forces the absolute patch (for the current thread), as if no memory was
  /// available near targets.
  pub(crate) static FORCE_FAR: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Creates a detour of `target` to `detour`.
///
/// # Safety
///
/// `target` must point to executable code.
pub(crate) unsafe fn build(target: *const (), detour: *const ()) -> Result<Hook> {
  let target = target as usize;
  let detour = detour as usize;

  if target % 4 != 0 || detour % 4 != 0 || memory::readable_len(target, 4) < 4 {
    return Err(Error::InvalidInstruction);
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
  if memory::readable_len(target, patch_offset + 4) < patch_offset + 4 {
    return Err(Error::InvalidInstruction);
  }

  #[cfg(test)]
  if FORCE_FAR.get() {
    // SAFETY: Forwarded from the caller.
    return unsafe { build_far(target, detour, patch_offset, relay_prefix) };
  }

  // SAFETY: Forwarded from the caller.
  match unsafe { build_near(target, detour, patch_offset, relay_prefix) } {
    // SAFETY: Forwarded from the caller.
    Err(Error::NoNearbyMemory) => unsafe { build_far(target, detour, patch_offset, relay_prefix) },
    result => result,
  }
}

/// Patches the target with a direct branch, to the detour or a nearby relay.
///
/// # Safety
///
/// See [`build`].
unsafe fn build_near(
  target: usize,
  detour: usize,
  patch_offset: usize,
  relay_prefix: Option<u32>,
) -> Result<Hook> {
  let patch_address = target + patch_offset;
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
  let branch = encode::b(patch_address, destination).ok_or(Error::NoNearbyMemory)?;

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

/// Patches the target with an absolute jump to the detour, and relocates the
/// replaced instructions to a trampoline allocated anywhere.
///
/// # Safety
///
/// See [`build`].
unsafe fn build_far(
  target: usize,
  detour: usize,
  patch_offset: usize,
  relay_prefix: Option<u32>,
) -> Result<Hook> {
  let patch_address = target + patch_offset;

  let mut patch = encode::Emitter::new(patch_address);
  if let Some(prefix) = relay_prefix {
    patch.push(prefix);
  }
  patch.absolute_jump(detour);
  let patched = patch.into_bytes();
  let patch_end = patch_address + patched.len();

  // Inspect the function beyond the patched instructions, for branches into them
  const SCAN_LEN: usize = 1024;
  let available = memory::readable_len(target, SCAN_LEN) & !3;
  if available < patch_end - target {
    return Err(Error::PatchAreaTooSmall);
  }
  // SAFETY: The range has been verified to be readable.
  let code = unsafe { core::slice::from_raw_parts(target as *const u32, available / 4) };
  let patched_count = (patch_end - target) / 4;

  // The function must not end within the patched instructions
  if code[..patched_count - 1].iter().any(|&instruction| encode::is_terminator(instruction)) {
    return Err(Error::PatchAreaTooSmall);
  }

  // The trampoline may be out of reach of a direct branch back, in which case
  // the absolute jump clobbers X16. It holds no value upon function entry, but
  // the replaced instructions must not assign it.
  if code[..patched_count].iter().any(|&instruction| encode::may_use_x16(instruction)) {
    return Err(Error::UnsupportedInstruction);
  }

  let replaced = (patch_address + 4)..patch_end;
  let branches_into_patch = code.iter().enumerate().any(|(index, &instruction)| {
    encode::branch_target(instruction, target + index * 4)
      .is_some_and(|destination| replaced.contains(&destination))
  });
  if branches_into_patch {
    return Err(Error::UnsupportedInstruction);
  }

  let mut trampoline =
    memory::allocate_near(target, usize::MAX, (patched_count + 1) * MAX_RELOCATED_LEN)?;
  let mut emitter = encode::Emitter::new(trampoline.address());
  for (index, &instruction) in code[..patched_count].iter().enumerate() {
    emitter.relocate(instruction, target + index * 4);
  }
  emitter.jump(patch_end);

  // SAFETY: The trampoline has just been allocated, and cannot be executing.
  unsafe { trampoline.write(&emitter.into_bytes())? };

  // SAFETY: The range has been verified to be readable.
  let original = unsafe { core::slice::from_raw_parts(patch_address as *const u8, patched.len()) };

  Ok(Hook {
    patch_address: patch_address as *mut u8,
    original: original.to_vec(),
    patched,
    trampoline,
    relay: None,
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

  /// Returns whether `instruction` unconditionally leaves the function (e.g.
  /// `B`, `BR` & `RET`, but not calls).
  pub fn is_terminator(instruction: u32) -> bool {
    let is_branch_immediate = instruction & 0xFC00_0000 == 0x1400_0000;
    // Unconditional branch (register), including pointer authentication variants
    let is_branch_register = instruction & 0xFE00_0000 == 0xD600_0000;
    let is_link = (instruction >> 21) & 0xF == 0b0001;
    is_branch_immediate || (is_branch_register && !is_link)
  }

  /// Returns whether `instruction` may reference X16 (conservatively, by
  /// inspecting every register field).
  pub fn may_use_x16(instruction: u32) -> bool {
    [0, 5, 10, 16].iter().any(|shift| (instruction >> shift) & 0x1F == X16)
  }

  /// Returns the destination of a PC-relative branch at `pc`, if any.
  pub fn branch_target(instruction: u32, pc: usize) -> Option<usize> {
    let target = |imm: u32, bits: u32| pc.wrapping_add((sign_extend(imm, bits) * 4) as usize);
    match instruction {
      // B & BL
      i if i & 0x7C00_0000 == 0x1400_0000 => Some(target(i & 0x3FF_FFFF, 26)),
      // B.cond, CBZ & CBNZ
      i if i & 0xFF00_0000 == 0x5400_0000 || i & 0x7E00_0000 == 0x3400_0000 => {
        Some(target((i >> 5) & 0x7_FFFF, 19))
      },
      // TBZ & TBNZ
      i if i & 0x7E00_0000 == 0x3600_0000 => Some(target((i >> 5) & 0x3FFF, 14)),
      _ => None,
    }
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

    /// Branches to `destination` using an absolute address.
    pub fn absolute_jump(&mut self, destination: usize) {
      // LDR X16, #8; BR X16; .quad destination
      self.push(ldr_literal(X16, 8));
      self.push(br(X16));
      self.literal(destination as u64);
    }

    /// Branches to `destination`.
    ///
    /// A direct branch is preferred, since indirect branches must target
    /// landing pads on BTI-guarded pages.
    pub fn jump(&mut self, destination: usize) {
      if let Some(branch) = b(self.pc(), destination) {
        self.push(branch);
      } else {
        self.absolute_jump(destination);
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
  fn classifies_terminators() {
    // b; br x16; ret; retaa; braaz x1
    for instruction in [0x1400_0010, 0xD61F_0200, 0xD65F_03C0, 0xD65F_0BFF, 0xD61F_083F] {
      assert!(is_terminator(instruction), "{instruction:#x}");
    }
    // bl; blr x8; b.eq; cbz x0; add x0, x1, x2
    for instruction in [0x9400_0010, 0xD63F_0100, 0x5400_0040, 0xB400_0040, 0x8B02_0020] {
      assert!(!is_terminator(instruction), "{instruction:#x}");
    }
  }

  #[test]
  fn resolves_branch_targets() {
    assert_eq!(branch_target(0x1400_0004, PC), Some(PC + 0x10)); // b
    assert_eq!(branch_target(0x97FF_FFFF, PC), Some(PC - 4)); // bl
    assert_eq!(branch_target(0x5400_0040, PC), Some(PC + 8)); // b.eq
    assert_eq!(branch_target(0xB400_0060, PC), Some(PC + 0xC)); // cbz x0
    assert_eq!(branch_target(0x3618_0081, PC), Some(PC + 0x10)); // tbz w1, #3
    assert_eq!(branch_target(0xD65F_03C0, PC), None); // ret
    assert_eq!(branch_target(0x1000_0082, PC), None); // adr
  }

  #[test]
  fn detects_x16_usage() {
    assert!(may_use_x16(0xD280_0030)); // mov x16, #1
    assert!(may_use_x16(0xF940_0210)); // ldr x16, [x16]
    assert!(may_use_x16(0x8B10_0020)); // add x0, x1, x16
    assert!(!may_use_x16(0x8B02_0020)); // add x0, x1, x2
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

/// Tests of the absolute patch (used when no memory is available nearby).
#[cfg(all(test, target_arch = "aarch64"))]
mod far_tests {
  use super::FORCE_FAR;
  use crate::{Error, RawDetour, Result};
  use core::arch::naked_asm;

  type Fn0 = unsafe extern "C" fn() -> i32;

  extern "C" fn ret10() -> i32 {
    10
  }

  /// Creates a detour using the absolute patch.
  fn far_detour(target: Fn0) -> Result<RawDetour> {
    FORCE_FAR.set(true);
    // SAFETY: Both functions share the same signature.
    let result = unsafe { RawDetour::new(target as *const (), ret10 as *const ()) };
    FORCE_FAR.set(false);
    result
  }

  /// Detours `target`, and asserts its return value before, during, and after.
  fn assert_far_detour(target: Fn0, result: i32) -> Result<()> {
    let hook = far_detour(target)?;
    // SAFETY: The fixtures are valid functions, not executed concurrently.
    unsafe {
      assert_eq!(target(), result);
      hook.enable()?;
      assert_eq!(target(), 10);
      let original: Fn0 = core::mem::transmute(hook.trampoline());
      assert_eq!(original(), result);
      hook.disable()?;
      assert_eq!(target(), result);
    }
    Ok(())
  }

  #[test]
  fn patches_absolute_jump() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn ret3() -> i32 {
      naked_asm!("mov w0, #1", "mov w1, #2", "add w0, w0, w1", "nop", "ret")
    }

    assert_far_detour(ret3, 3)
  }

  #[test]
  fn relocates_pc_relative_instructions() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn literal_ret77() -> i32 {
      naked_asm!("ldr w0, 2f", "nop", "nop", "nop", "ret", "2:", ".word 77")
    }

    assert_far_detour(literal_ret77, 77)
  }

  #[test]
  fn preserves_pointer_authentication() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn pac_ret5() -> i32 {
      // paciasp; ...; autiasp; ret
      naked_asm!("hint #25", "mov w0, #5", "nop", "nop", "nop", "nop", "hint #29", "ret")
    }

    assert_far_detour(pac_ret5, 5)
  }

  #[test]
  fn rejects_branches_into_patch() {
    #[unsafe(naked)]
    unsafe extern "C" fn branch_into_patch() -> i32 {
      naked_asm!("cbz x0, 2f", "nop", "2:", "mov w0, #1", "nop", "ret")
    }

    assert!(matches!(far_detour(branch_into_patch), Err(Error::UnsupportedInstruction)));
  }

  #[test]
  fn rejects_x16_usage() {
    #[unsafe(naked)]
    unsafe extern "C" fn uses_x16() -> i32 {
      naked_asm!("mov x16, #1", "mov w0, w16", "nop", "nop", "ret")
    }

    assert!(matches!(far_detour(uses_x16), Err(Error::UnsupportedInstruction)));
  }

  /// The case this patch exists for: a function in the dyld shared cache.
  #[test]
  #[cfg(target_vendor = "apple")]
  fn patches_shared_cache_function() -> Result<()> {
    extern "C" fn fake_pid() -> libc::pid_t {
      -7
    }

    // SAFETY: The symbol name is null-terminated.
    let getpid = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"getpid".as_ptr()) };
    FORCE_FAR.set(true);
    // SAFETY: Both functions share the same signature.
    let hook = unsafe { RawDetour::new(getpid.cast_const().cast(), fake_pid as *const ()) };
    FORCE_FAR.set(false);
    let hook = hook?;

    // SAFETY: `getpid` has no preconditions.
    unsafe {
      let expected = libc::getpid();
      hook.enable()?;
      assert_eq!(libc::getpid(), -7);
      let original: extern "C" fn() -> libc::pid_t = core::mem::transmute(hook.trampoline());
      assert_eq!(original(), expected);
      hook.disable()?;
      assert_eq!(libc::getpid(), expected);
    }
    Ok(())
  }

  #[test]
  fn rejects_small_functions() {
    #[unsafe(naked)]
    unsafe extern "C" fn tiny() -> i32 {
      naked_asm!("mov w0, #5", "ret", "mov w0, #6", "ret", "nop")
    }

    assert!(matches!(far_detour(tiny), Err(Error::PatchAreaTooSmall)));
  }
}
