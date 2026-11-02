//! Differential tests of the relocator, executing random PC-relative
//! instructions in place and relocated, and comparing the resulting state.
//!
//! Instructions run within a code region in which every 4-byte slot is
//! `JAL RA, <stub>`. The stub saves all registers and returns to the harness,
//! so the slot reached (`RA - 4`) identifies where execution continued.
//!
//! Instructions are placed at a slot, or 2 bytes into one (preceded by
//! `C.NOP`), and padded with `C.NOP` up to the next slot, which is reached
//! upon falling through. Likewise, relocated code is placed at 4-byte and
//! 2-byte aligned addresses.
//!
//! The seed and the number of iterations per class may be overridden with
//! `DETOUR_RELOCATE_SEED` and `DETOUR_RELOCATE_ITERATIONS`.

use super::encode::{self, Emitter};
use core::arch::naked_asm;
use core::mem::offset_of;
use core::ops::Range;
use std::boxed::Box;
use std::format;
use std::string::String;
use std::vec::Vec;

const DEFAULT_SEED: u64 = 0x0D37_0A64;
const DEFAULT_ITERATIONS: usize = 10_000;

/// The size of the region containing the original instructions.
const REGION_LEN: usize = 0x20_0000;
/// The offset of the exit stub, within reach of `JAL` from every slot.
const STUB: usize = 0x10_0000;
const STUB_LEN: usize = 32;
/// The range of slots (relative to the region) for instructions & targets,
/// excluding the stub.
const SLOTS: Range<usize> = 0x1000..0x1F_0000;
/// The offset at which relocated instructions are placed within reach.
const NEAR: usize = 0x1F_0000;
/// The size of the region containing relocated instructions out of reach.
const FAR_LEN: usize = 0x1_0000;
/// The minimum distance of relocated instructions placed out of reach (of
/// `AUIPC`, i.e. ±2 GiB).
const FAR_DISTANCE: usize = 0x1_0000_0000;

/// The scratch register, excluded from operands & comparisons.
const T1: u32 = encode::T1;
const RA: u32 = 1;
const SP: u32 = 2;

/// Registers which may be used as operands; GP & TP are left untouched, T1
/// is clobbered (and the exit stub uses it), and RA is overwritten by the
/// exit stub.
const REGISTERS: &[u32] = &[
  0, 1, 2, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
];

/// The architectural state, loaded before and saved after execution.
#[repr(C)]
#[derive(Clone, Default)]
struct Context {
  /// X0–X31; X0 and the registers excluded from [`REGISTERS`] (but SP & RA)
  /// are neither loaded nor saved, and SP is saved only.
  x: [u64; 32],
  /// The address at which execution starts.
  entry: u64,
  /// The harness's stack pointer.
  host_sp: u64,
  /// The address of [`exit`], loaded by the stub.
  exit: u64,
}

const _: () = {
  assert!(offset_of!(Context, entry) == 256);
  assert!(offset_of!(Context, host_sp) == 264);
  assert!(offset_of!(Context, exit) == 272);
};

/// Loads `input` and branches to its entry, until the exit stub saves the
/// state to `output` (which the stub refers to) and returns.
#[unsafe(naked)]
unsafe extern "C" fn enter(input: *const Context, output: *mut Context) {
  naked_asm!(
    "addi sp, sp, -128",
    "sd ra, 0(sp)",
    "sd gp, 8(sp)",
    "sd tp, 16(sp)",
    "sd s0, 24(sp)",
    "sd s1, 32(sp)",
    "sd s2, 40(sp)",
    "sd s3, 48(sp)",
    "sd s4, 56(sp)",
    "sd s5, 64(sp)",
    "sd s6, 72(sp)",
    "sd s7, 80(sp)",
    "sd s8, 88(sp)",
    "sd s9, 96(sp)",
    "sd s10, 104(sp)",
    "sd s11, 112(sp)",
    "sd sp, 264(a1)",
    "mv t1, a0",
    "ld x1, 8(t1)",
    "ld x5, 40(t1)",
    "ld x7, 56(t1)",
    "ld x8, 64(t1)",
    "ld x9, 72(t1)",
    "ld x10, 80(t1)",
    "ld x11, 88(t1)",
    "ld x12, 96(t1)",
    "ld x13, 104(t1)",
    "ld x14, 112(t1)",
    "ld x15, 120(t1)",
    "ld x16, 128(t1)",
    "ld x17, 136(t1)",
    "ld x18, 144(t1)",
    "ld x19, 152(t1)",
    "ld x20, 160(t1)",
    "ld x21, 168(t1)",
    "ld x22, 176(t1)",
    "ld x23, 184(t1)",
    "ld x24, 192(t1)",
    "ld x25, 200(t1)",
    "ld x26, 208(t1)",
    "ld x27, 216(t1)",
    "ld x28, 224(t1)",
    "ld x29, 232(t1)",
    "ld x30, 240(t1)",
    "ld x31, 248(t1)",
    "ld t1, 256(t1)",
    "jr t1",
  )
}

/// Saves the state to the context in T1 (with RA saved by the stub), and
/// returns from [`enter`].
#[unsafe(naked)]
unsafe extern "C" fn exit() {
  naked_asm!(
    "sd x2, 16(t1)",
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
    "ld sp, 264(t1)",
    "ld ra, 0(sp)",
    "ld gp, 8(sp)",
    "ld tp, 16(sp)",
    "ld s0, 24(sp)",
    "ld s1, 32(sp)",
    "ld s2, 40(sp)",
    "ld s3, 48(sp)",
    "ld s4, 56(sp)",
    "ld s5, 64(sp)",
    "ld s6, 72(sp)",
    "ld s7, 80(sp)",
    "ld s8, 88(sp)",
    "ld s9, 96(sp)",
    "ld s10, 104(sp)",
    "ld s11, 112(sp)",
    "addi sp, sp, 128",
    "ret",
  )
}

/// Makes code written by this thread visible to its instruction fetches.
fn fence_i() {
  // SAFETY: `FENCE.I` only synchronizes the instruction & data streams.
  unsafe { core::arch::asm!(".insn i 0x0F, 1, x0, x0, 0", options(nostack, preserves_flags)) };
}

/// Encoders, independent of the relocator's.
mod insn {
  fn bit(value: u32, index: u32) -> u32 {
    (value >> index) & 1
  }

  fn bits(value: u32, high: u32, low: u32) -> u32 {
    (value >> low) & ((1 << (high - low + 1)) - 1)
  }

  /// `C.NOP`
  pub const C_NOP: u16 = 0x0001;

  pub fn auipc(rd: u32, imm: u32) -> u32 {
    imm << 12 | rd << 7 | 0x17
  }

  pub fn ld(rd: u32, rs1: u32, imm: u32) -> u32 {
    imm << 20 | rs1 << 15 | 0b011 << 12 | rd << 7 | 0x03
  }

  pub fn sd(rs2: u32, rs1: u32, imm: u32) -> u32 {
    bits(imm, 11, 5) << 25 | rs2 << 20 | rs1 << 15 | 0b011 << 12 | bits(imm, 4, 0) << 7 | 0x23
  }

  pub fn jalr(rd: u32, rs1: u32, imm: u32) -> u32 {
    imm << 20 | rs1 << 15 | rd << 7 | 0x67
  }

  pub fn jal(rd: u32, offset: i64) -> u32 {
    assert!((-(1 << 20)..(1 << 20)).contains(&offset) && offset % 2 == 0, "{offset:#x}");
    let imm = offset as u32;
    bit(imm, 20) << 31 | bits(imm, 10, 1) << 21 | bit(imm, 11) << 20 | bits(imm, 19, 12) << 12 | rd << 7 | 0x6F
  }

  pub fn branch(funct3: u32, rs1: u32, rs2: u32, offset: i64) -> u32 {
    assert!((-(1 << 12)..(1 << 12)).contains(&offset) && offset % 2 == 0, "{offset:#x}");
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

  pub fn c_j(offset: i64) -> u32 {
    assert!((-(1 << 11)..(1 << 11)).contains(&offset) && offset % 2 == 0, "{offset:#x}");
    let imm = offset as u32;
    0b101 << 13
      | bit(imm, 11) << 12
      | bit(imm, 4) << 11
      | bits(imm, 9, 8) << 9
      | bit(imm, 10) << 8
      | bit(imm, 6) << 7
      | bit(imm, 7) << 6
      | bits(imm, 3, 1) << 3
      | bit(imm, 5) << 2
      | 0b01
  }

  /// `C.BEQZ` (`funct3 = 0b110`) or `C.BNEZ` (`0b111`), with `rs1` in X8–X15.
  pub fn c_branch(funct3: u32, rs1: u32, offset: i64) -> u32 {
    assert!((-(1 << 8)..(1 << 8)).contains(&offset) && offset % 2 == 0, "{offset:#x}");
    let imm = offset as u32;
    funct3 << 13
      | bit(imm, 8) << 12
      | bits(imm, 4, 3) << 10
      | (rs1 - 8) << 7
      | bits(imm, 7, 6) << 5
      | bits(imm, 2, 1) << 3
      | bit(imm, 5) << 2
      | 0b01
  }
}

/// A small deterministic PRNG (SplitMix64).
struct Rng(u64);

impl Rng {
  fn next(&mut self) -> u64 {
    self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = self.0;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
  }

  /// Returns a value in `0..n`.
  fn below(&mut self, n: u64) -> u64 {
    self.next() % n
  }

  fn register(&mut self) -> u32 {
    REGISTERS[self.below(REGISTERS.len() as u64) as usize]
  }
}

/// Returns an environment variable parsed as an integer (hexadecimal with a
/// `0x` prefix), or `default`.
fn env_or(name: &str, default: u64) -> u64 {
  std::env::var(name)
    .ok()
    .and_then(|value| match value.strip_prefix("0x") {
      Some(hex) => u64::from_str_radix(hex, 16).ok(),
      None => value.parse().ok(),
    })
    .unwrap_or(default)
}

/// An executable mapping, filled with `JAL RA, <stub>` slots.
struct Region {
  base: usize,
  len: usize,
  stub: usize,
}

impl Region {
  /// Maps a region at `hint` (if available), with an exit stub at `stub`
  /// (relative to the region) that saves the state to `output`.
  fn map(hint: usize, len: usize, stub: usize, output: *mut Context) -> Option<Self> {
    // SAFETY: Anonymous mappings without `MAP_FIXED` never replace memory.
    let address = unsafe {
      libc::mmap(
        hint as *mut libc::c_void,
        len,
        libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        libc::MAP_PRIVATE | libc::MAP_ANON,
        -1,
        0,
      )
    };
    if address == libc::MAP_FAILED {
      return None;
    }

    let region = Region {
      base: address as usize,
      len,
      stub: address as usize + stub,
    };
    region.reset(region.base + 0x1000, region.base + len);
    let [output_lo, output_hi] = split(output as u64);
    // AUIPC T1, 0; LD T1, 24(T1); SD RA, 8(T1); LD RA, <exit>(T1); JR RA;
    // NOP; .dword output
    let code = [
      insn::auipc(T1, 0),
      insn::ld(T1, T1, 24),
      insn::sd(RA, T1, 8),
      insn::ld(RA, T1, offset_of!(Context, exit) as u32),
      insn::jalr(0, RA, 0),
      encode::NOP,
      output_lo,
      output_hi,
    ];
    region.write(region.stub, &words(&code));
    Some(region)
  }

  /// Maps a region at least `distance` bytes away from `near` (above or
  /// below it, since user space may end shortly above it, e.g. with Sv39).
  fn map_far(near: &Region, distance: usize, len: usize, output: *mut Context) -> Self {
    for step in 0..256 {
      let above = near.base + near.len + distance + step * 0x100_0000;
      let below = near.base.saturating_sub(distance + len + step * 0x100_0000);
      for hint in [above, below] {
        if let Some(region) = Region::map(hint, len, 0, output) {
          if region.base.abs_diff(near.base) >= distance && near.base.abs_diff(region.base + len) >= distance {
            return region;
          }
        }
      }
    }
    panic!("could not map a distant region");
  }

  fn contains(&self, address: usize) -> bool {
    (self.base..self.base + self.len).contains(&address)
  }

  /// Returns whether `address` is a slot available for instructions &
  /// targets.
  fn is_slot(&self, address: usize) -> bool {
    let offset = address.wrapping_sub(self.base);
    address % 4 == 0 && SLOTS.contains(&offset) && !(self.stub..self.stub + STUB_LEN).contains(&address)
  }

  /// Writes `code` at `address`.
  fn write(&self, address: usize, code: &[u8]) {
    assert!(self.contains(address) && self.contains(address + code.len() - 1));
    // SAFETY: The range is within the mapping, which is writable, and not
    // being executed.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u8, code.len()) };
    fence_i();
  }

  /// Restores the slots spanning `start..end`.
  fn reset(&self, start: usize, end: usize) {
    let start = start & !3;
    let slots: Vec<u32> = (start..end)
      .step_by(4)
      .map(|address| insn::jal(RA, self.stub as i64 - address as i64))
      .collect();
    self.write(start, &words(&slots));
  }
}

impl Drop for Region {
  fn drop(&mut self) {
    // SAFETY: The region is no longer referenced.
    unsafe { libc::munmap(self.base as *mut libc::c_void, self.len) };
  }
}

fn split(value: u64) -> [u32; 2] {
  [value as u32, (value >> 32) as u32]
}

fn words(code: &[u32]) -> Vec<u8> {
  code.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// Returns the bytes of a 2 or 4-byte `instruction`.
fn bytes(instruction: u32) -> Vec<u8> {
  let len = if encode::is_compressed(instruction) { 2 } else { 4 };
  instruction.to_le_bytes()[..len].to_vec()
}

/// The slots occupied by an instruction of `len` bytes at `pc` (including
/// its padding); the end is the slot reached upon falling through.
fn occupied(pc: usize, len: usize) -> Range<usize> {
  (pc & !3)..(pc + len).next_multiple_of(4)
}

/// A random instance of an instruction class.
struct Instance {
  instruction: u32,
  /// The target of a call (which must return via its link register).
  call: Option<(usize, u32)>,
}

/// Generates a random instance of an instruction class, located at `pc`.
type Generator = fn(&mut Rng, &mut Context, &Region, usize) -> Instance;

/// Returns a random slot within `-range..range` bytes of an instruction of
/// `len` bytes at `pc`, avoiding the slots it occupies (which would loop).
fn target(rng: &mut Rng, region: &Region, pc: usize, len: usize, range: usize) -> usize {
  let low = (region.base + SLOTS.start).max(pc - range).next_multiple_of(4);
  let high = (region.base + SLOTS.end - 4).min(pc + range - 2);
  let occupied = occupied(pc, len);
  loop {
    let target = low + rng.below(((high - low) / 4 + 1) as u64) as usize * 4;
    if region.is_slot(target) && !occupied.contains(&target) {
      return target;
    }
  }
}

fn offset(pc: usize, target: usize) -> i64 {
  target as i64 - pc as i64
}

/// Sets the operands of a conditional branch, equal or zero with a high
/// probability (so both outcomes are frequent).
fn operands(rng: &mut Rng, input: &mut Context, rs1: u32, rs2: u32) {
  let value = |rng: &mut Rng| match rng.below(4) {
    0 => 0,
    1 => rng.next() >> rng.below(64),
    2 => (rng.next() as i64 >> rng.below(64)) as u64,
    _ => rng.next(),
  };
  for rs in [rs1, rs2] {
    if rs != 0 && rs != SP {
      input.x[rs as usize] = value(rng);
    }
  }
  if rs1 != 0 && rs1 != SP && rng.below(3) == 0 {
    input.x[rs1 as usize] = if rs2 == 0 || rs2 == SP { 0 } else { input.x[rs2 as usize] };
  }
}

fn address(rng: &mut Rng, _: &mut Context, _: &Region, _: usize) -> Instance {
  // `AUIPC X0` (a hint) is copied; RA is overwritten by the exit stub
  let rd = loop {
    let rd = rng.register();
    if rd != RA {
      break rd;
    }
  };
  let imm = rng.next() as u32 & 0xF_FFFF;
  Instance {
    instruction: insn::auipc(rd, imm),
    call: None,
  }
}

fn jump(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> Instance {
  let target = target(rng, region, pc, 4, 1 << 20);
  Instance {
    instruction: insn::jal(0, offset(pc, target)),
    call: None,
  }
}

fn call(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> Instance {
  let rd = loop {
    let rd = rng.register();
    if rd != 0 {
      break rd;
    }
  };
  let end = occupied(pc, 4).end;
  let destination = loop {
    let destination = target(rng, region, pc, 4, 1 << 20);
    if destination != end {
      break destination;
    }
  };
  Instance {
    instruction: insn::jal(rd, offset(pc, destination)),
    call: Some((destination, insn::jalr(0, rd, 0))),
  }
}

fn branch(rng: &mut Rng, input: &mut Context, region: &Region, pc: usize) -> Instance {
  let target = target(rng, region, pc, 4, 1 << 12);
  let funct3 = [0b000, 0b001, 0b100, 0b101, 0b110, 0b111][rng.below(6) as usize];
  let (rs1, rs2) = (rng.register(), rng.register());
  operands(rng, input, rs1, rs2);
  Instance {
    instruction: insn::branch(funct3, rs1, rs2, offset(pc, target)),
    call: None,
  }
}

fn compressed_jump(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> Instance {
  let target = target(rng, region, pc, 2, 1 << 11);
  Instance {
    instruction: insn::c_j(offset(pc, target)),
    call: None,
  }
}

fn compressed_branch(rng: &mut Rng, input: &mut Context, region: &Region, pc: usize) -> Instance {
  let target = target(rng, region, pc, 2, 1 << 8);
  let rs1 = 8 + rng.below(8) as u32;
  operands(rng, input, rs1, 0);
  let funct3 = 0b110 | rng.below(2) as u32;
  Instance {
    instruction: insn::c_branch(funct3, rs1, offset(pc, target)),
    call: None,
  }
}

/// Executes code at `entry`, returning the resulting state.
fn execute(input: &Context, output: &mut Context, entry: usize) -> Context {
  let mut input = input.clone();
  input.entry = entry as u64;
  output.exit = exit as *const () as u64;
  // SAFETY: The entry leads to the exit stub, which saves to `output`.
  unsafe { enter(&input, output) };
  output.clone()
}

/// Returns the differences between the state after the original (`expected`)
/// and the relocated code (`actual`), ignoring `ignored` (a call's link
/// register, which refers to the relocated code).
fn differences(expected: &Context, actual: &Context, ignored: Option<u32>) -> Vec<String> {
  let mut differences = Vec::new();
  let slot = |context: &Context| context.x[RA as usize].wrapping_sub(4);
  if slot(expected) != slot(actual) {
    differences.push(format!("slot: {:#x} != {:#x}", slot(expected), slot(actual)));
  }
  for &index in REGISTERS.iter().filter(|&&index| index != 0 && index != RA && Some(index) != ignored) {
    let index = index as usize;
    if expected.x[index] != actual.x[index] {
      differences.push(format!("x{index}: {:#x} != {:#x}", expected.x[index], actual.x[index]));
    }
  }
  differences
}

/// Runs random instances of a class (of `len` bytes), relocated both within
/// and beyond the reach of `AUIPC`.
///
/// Returns the number of instances which fell through, and which branched.
fn check(name: &str, len: usize, generate: Generator) -> [usize; 2] {
  let seed = env_or("DETOUR_RELOCATE_SEED", DEFAULT_SEED);
  let iterations = env_or("DETOUR_RELOCATE_ITERATIONS", DEFAULT_ITERATIONS as u64) as usize;
  let mut rng = Rng(seed ^ name.bytes().fold(0_u64, |hash, byte| hash.rotate_left(8) ^ u64::from(byte)));

  let mut output = Box::new(Context::default());
  let region = Region::map(0, REGION_LEN, STUB, &mut *output).expect("could not map a region");
  let far = Region::map_far(&region, FAR_DISTANCE, FAR_LEN, &mut *output);

  let mut outcomes = [0; 2];
  for iteration in 0..iterations {
    let mut input = Context {
      x: core::array::from_fn(|_| rng.next()),
      ..Context::default()
    };

    // At a slot, or 2 bytes into one (preceded by `C.NOP`)
    let (pc, slots) = loop {
      let slot = region.base + SLOTS.start + rng.below(((SLOTS.end - SLOTS.start) / 4) as u64) as usize * 4;
      let pc = slot + 2 * rng.below(2) as usize;
      let slots = occupied(pc, len);
      if (slots.start..=slots.end).step_by(4).all(|slot| region.is_slot(slot)) {
        break (pc, slots);
      }
    };
    let Instance { instruction, call } = generate(&mut rng, &mut input, &region, pc);
    assert_eq!(bytes(instruction).len(), len);

    if let Some((destination, ret)) = call {
      region.write(destination, &ret.to_le_bytes());
    }
    let mut code = Vec::new();
    if pc % 4 != 0 {
      code.extend_from_slice(&insn::C_NOP.to_le_bytes());
    }
    code.extend_from_slice(&bytes(instruction));
    if code.len() % 4 != 0 {
      code.extend_from_slice(&insn::C_NOP.to_le_bytes());
    }
    region.write(slots.start, &code);
    let expected = execute(&input, &mut output, pc);
    outcomes[usize::from(expected.x[RA as usize] != (slots.end + 4) as u64)] += 1;

    for (placement, start) in [(&region, region.base + NEAR), (&far, far.base + 0x1000)] {
      let base = start + 2 * rng.below(2) as usize;
      let mut emitter = Emitter::new(base);
      emitter.relocate(instruction, pc);
      emitter.jump(pc + len);
      let code = emitter.into_bytes();
      placement.write(base, &code);
      let actual = execute(&input, &mut output, base);
      placement.reset(start, base + code.len());

      let ignored = call.map(|_| (instruction >> 7) & 0x1F);
      let differences = differences(&expected, &actual, ignored);
      assert!(
        differences.is_empty(),
        "{name} #{iteration} (seed {seed:#x}): {:#010x} at {:#x}, relocated to {base:#x} as {:02x?}:\n{}",
        instruction,
        pc,
        code,
        differences.join("\n"),
      );
    }

    region.reset(slots.start, slots.end);
    if let Some((destination, _)) = call {
      region.reset(destination, destination + 4);
    }
  }
  outcomes
}

/// Runs a conditional class, asserting that both outcomes are exercised.
fn check_conditional(name: &str, len: usize, generate: Generator) {
  let [fallthrough, branched] = check(name, len, generate);
  assert!(fallthrough > 0 && branched > 0, "{name}: {fallthrough} fell through, {branched} branched");
}

#[test]
fn relocates_address_calculations() {
  check("auipc", 4, address);
}

#[test]
fn relocates_jumps() {
  check("jal x0", 4, jump);
}

#[test]
fn relocates_calls() {
  check("jal", 4, call);
}

#[test]
fn relocates_branches() {
  check_conditional("b<cond>", 4, branch);
}

#[test]
fn relocates_compressed_jumps() {
  check("c.j", 2, compressed_jump);
}

#[test]
fn relocates_compressed_branches() {
  check_conditional("c.b<cond>z", 2, compressed_branch);
}
