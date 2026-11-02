//! Differential tests of the relocator, executing random PC-relative
//! instructions in place and relocated, and comparing the resulting state.
//!
//! Instructions run within a code region in which every slot is `BL <stub>`.
//! The stub saves all registers and returns to the harness, so the slot
//! reached (`X30 - 4`) identifies where execution continued.
//!
//! The seed and the number of iterations per class may be overridden with
//! `DETOUR_RELOCATE_SEED` and `DETOUR_RELOCATE_ITERATIONS`.

use super::encode::{self, Emitter};
use core::arch::naked_asm;
use core::mem::offset_of;
use std::boxed::Box;
use std::format;
use std::string::String;
use std::vec::Vec;

const DEFAULT_SEED: u64 = 0x0D37_0A64;
const DEFAULT_ITERATIONS: usize = 10_000;

/// The size of the region containing the original instructions.
const REGION_LEN: usize = 0x40_0000;
/// The range of slots (relative to the region) for instructions & targets.
const SLOTS: core::ops::Range<usize> = 0x1000..0x38_0000;
/// The offset at which relocated instructions are placed within reach.
const NEAR: usize = 0x38_0000;
/// The minimum distance of relocated instructions placed out of reach.
const FAR_DISTANCE: usize = 0x1000_0000;

const RET: u32 = 0xD65F_03C0;

/// Registers which may be used as operands; X16 & X17 are clobbered, X18 is
/// reserved by some platforms, and X30 is overwritten by the exit stub.
const REGISTERS: &[u32] = &[
  0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
  29, 31,
];

/// The architectural state, loaded before and saved after execution.
#[repr(C, align(16))]
#[derive(Clone, Default)]
struct Context {
  x: [u64; 31],
  sp: u64,
  nzcv: u64,
  _padding: u64,
  q: [u128; 32],
  /// The address at which execution starts.
  entry: u64,
  /// The harness's stack pointer.
  host_sp: u64,
}

const _: () = {
  assert!(offset_of!(Context, sp) == 248);
  assert!(offset_of!(Context, nzcv) == 256);
  assert!(offset_of!(Context, q) == 272);
  assert!(offset_of!(Context, entry) == 784);
  assert!(offset_of!(Context, host_sp) == 792);
};

/// Loads `input` and branches to its entry, until the exit stub saves the
/// state to `output` (which the stub refers to) and returns.
#[unsafe(naked)]
unsafe extern "C" fn enter(input: *const Context, output: *mut Context) {
  naked_asm!(
    "sub sp, sp, #160",
    "stp x19, x20, [sp, #0]",
    "stp x21, x22, [sp, #16]",
    "stp x23, x24, [sp, #32]",
    "stp x25, x26, [sp, #48]",
    "stp x27, x28, [sp, #64]",
    "stp x29, x30, [sp, #80]",
    "stp d8, d9, [sp, #96]",
    "stp d10, d11, [sp, #112]",
    "stp d12, d13, [sp, #128]",
    "stp d14, d15, [sp, #144]",
    "mov x9, sp",
    "str x9, [x1, #792]",
    "mov x16, x0",
    "add x17, x16, #272",
    "ldp q0, q1, [x17, #0]",
    "ldp q2, q3, [x17, #32]",
    "ldp q4, q5, [x17, #64]",
    "ldp q6, q7, [x17, #96]",
    "ldp q8, q9, [x17, #128]",
    "ldp q10, q11, [x17, #160]",
    "ldp q12, q13, [x17, #192]",
    "ldp q14, q15, [x17, #224]",
    "ldp q16, q17, [x17, #256]",
    "ldp q18, q19, [x17, #288]",
    "ldp q20, q21, [x17, #320]",
    "ldp q22, q23, [x17, #352]",
    "ldp q24, q25, [x17, #384]",
    "ldp q26, q27, [x17, #416]",
    "ldp q28, q29, [x17, #448]",
    "ldp q30, q31, [x17, #480]",
    "ldr x17, [x16, #256]",
    "msr nzcv, x17",
    "ldp x0, x1, [x16, #0]",
    "ldp x2, x3, [x16, #16]",
    "ldp x4, x5, [x16, #32]",
    "ldp x6, x7, [x16, #48]",
    "ldp x8, x9, [x16, #64]",
    "ldp x10, x11, [x16, #80]",
    "ldp x12, x13, [x16, #96]",
    "ldp x14, x15, [x16, #112]",
    "ldr x19, [x16, #152]",
    "ldp x20, x21, [x16, #160]",
    "ldp x22, x23, [x16, #176]",
    "ldp x24, x25, [x16, #192]",
    "ldp x26, x27, [x16, #208]",
    "ldp x28, x29, [x16, #224]",
    "ldr x30, [x16, #240]",
    "ldr x17, [x16, #784]",
    "br x17",
  )
}

/// Saves the state to the context in X17, and returns from [`enter`].
#[unsafe(naked)]
unsafe extern "C" fn exit() {
  naked_asm!(
    // BTI JC
    "hint #38",
    "stp x0, x1, [x17, #0]",
    "stp x2, x3, [x17, #16]",
    "stp x4, x5, [x17, #32]",
    "stp x6, x7, [x17, #48]",
    "stp x8, x9, [x17, #64]",
    "stp x10, x11, [x17, #80]",
    "stp x12, x13, [x17, #96]",
    "stp x14, x15, [x17, #112]",
    "str x19, [x17, #152]",
    "stp x20, x21, [x17, #160]",
    "stp x22, x23, [x17, #176]",
    "stp x24, x25, [x17, #192]",
    "stp x26, x27, [x17, #208]",
    "stp x28, x29, [x17, #224]",
    "str x30, [x17, #240]",
    "mov x16, sp",
    "str x16, [x17, #248]",
    "mrs x16, nzcv",
    "str x16, [x17, #256]",
    "add x16, x17, #272",
    "stp q0, q1, [x16, #0]",
    "stp q2, q3, [x16, #32]",
    "stp q4, q5, [x16, #64]",
    "stp q6, q7, [x16, #96]",
    "stp q8, q9, [x16, #128]",
    "stp q10, q11, [x16, #160]",
    "stp q12, q13, [x16, #192]",
    "stp q14, q15, [x16, #224]",
    "stp q16, q17, [x16, #256]",
    "stp q18, q19, [x16, #288]",
    "stp q20, q21, [x16, #320]",
    "stp q22, q23, [x16, #352]",
    "stp q24, q25, [x16, #384]",
    "stp q26, q27, [x16, #416]",
    "stp q28, q29, [x16, #448]",
    "stp q30, q31, [x16, #480]",
    "ldr x16, [x17, #792]",
    "mov sp, x16",
    "ldp d14, d15, [sp, #144]",
    "ldp d12, d13, [sp, #128]",
    "ldp d10, d11, [sp, #112]",
    "ldp d8, d9, [sp, #96]",
    "ldp x29, x30, [sp, #80]",
    "ldp x27, x28, [sp, #64]",
    "ldp x25, x26, [sp, #48]",
    "ldp x23, x24, [sp, #32]",
    "ldp x21, x22, [sp, #16]",
    "ldp x19, x20, [sp, #0]",
    "add sp, sp, #160",
    "ret",
  )
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

  fn bool(&mut self) -> bool {
    self.next() & 1 != 0
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

/// An executable mapping, filled with `BL <stub>` slots.
struct Region {
  base: usize,
  len: usize,
}

impl Region {
  /// Maps a region at `hint` (if available), with an exit stub that saves
  /// the state to `output`.
  fn map(hint: usize, len: usize, output: *mut Context) -> Option<Self> {
    #[cfg(target_vendor = "apple")]
    let flags = libc::MAP_JIT;
    #[cfg(not(target_vendor = "apple"))]
    let flags = 0;

    // SAFETY: Anonymous mappings without `MAP_FIXED` never replace memory.
    let address = unsafe {
      libc::mmap(
        hint as *mut libc::c_void,
        len,
        libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        libc::MAP_PRIVATE | libc::MAP_ANON | flags,
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
    };
    let [exit_lo, exit_hi] = split(exit as *const () as u64);
    let [output_lo, output_hi] = split(output as u64);
    // LDR X17, #16; LDR X16, #20; BR X16; NOP; .quad output; .quad exit
    let stub = [
      0x5800_0091,
      0x5800_00B0,
      0xD61F_0200,
      encode::NOP,
      output_lo,
      output_hi,
      exit_lo,
      exit_hi,
    ];
    region.write(region.base, &stub);
    region.reset(region.base + 0x1000, (len - 0x1000) / 4);
    Some(region)
  }

  /// Maps a region at least `distance` bytes away from `near`.
  fn map_far(near: &Region, distance: usize, len: usize, output: *mut Context) -> Self {
    let mut hint = near.base + near.len + distance;
    for _ in 0..256 {
      if let Some(region) = Region::map(hint, len, output) {
        if region.base.abs_diff(near.base) >= distance && near.base.abs_diff(region.base + len) >= distance {
          return region;
        }
      }
      hint += 0x100_0000;
    }
    panic!("could not map a distant region");
  }

  fn contains(&self, address: usize) -> bool {
    (self.base..self.base + self.len).contains(&address)
  }

  /// Writes `code` at `address`.
  fn write(&self, address: usize, code: &[u32]) {
    assert!(self.contains(address) && self.contains(address + code.len() * 4 - 1));
    // SAFETY: The range is within the mapping, which is writable (for this
    // thread, in the case of JIT), and not being executed.
    unsafe {
      #[cfg(target_vendor = "apple")]
      libc::pthread_jit_write_protect_np(0);
      core::ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u32, code.len());
      #[cfg(target_vendor = "apple")]
      libc::pthread_jit_write_protect_np(1);
    }
    crate::memory::flush_instruction_cache(address as *const u8, code.len() * 4);
  }

  /// Restores `count` slots at `address`.
  fn reset(&self, address: usize, count: usize) {
    let slots: Vec<u32> = (0..count)
      .map(|index| encode::bl(address + index * 4, self.base).unwrap())
      .collect();
    self.write(address, &slots);
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

/// Generates a random instance of an instruction class, located at `pc`, and
/// the target of a `BL` (which must return).
type Generator = fn(&mut Rng, &mut Context, &Region, usize) -> (u32, Option<usize>);

/// Returns a random target within `±range` bytes of `pc`, and within the
/// slots (with `size` readable bytes), avoiding `pc` itself (which would loop).
fn target(rng: &mut Rng, region: &Region, pc: usize, range: usize, size: usize) -> usize {
  let low = (region.base + SLOTS.start).max(pc.saturating_sub(range));
  let high = (region.base + SLOTS.end - size).min(pc + range - 4);
  loop {
    let target = low + rng.below(((high - low) / 4 + 1) as u64) as usize * 4;
    if target != pc {
      return target;
    }
  }
}

/// Returns the immediate of a PC-relative offset, truncated to `bits`.
fn imm(pc: usize, target: usize, bits: u32) -> u32 {
  ((target.wrapping_sub(pc) as i64 >> 2) as u32) & ((1 << bits) - 1)
}

fn branch(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let target = target(rng, region, pc, 1 << 27, 4);
  (0x1400_0000 | imm(pc, target, 26), None)
}

fn branch_with_link(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let mut destination = pc + 4;
  while destination == pc + 4 {
    destination = target(rng, region, pc, 1 << 27, 4);
  }
  (0x9400_0000 | imm(pc, destination, 26), Some(destination))
}

fn conditional_branch(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let target = target(rng, region, pc, 1 << 20, 4);
  let condition = rng.below(16) as u32;
  (0x5400_0000 | (imm(pc, target, 19) << 5) | condition, None)
}

fn compare_and_branch(rng: &mut Rng, input: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let target = target(rng, region, pc, 1 << 20, 4);
  let rt = rng.register();
  let sf = rng.below(2) as u32;
  let op = rng.below(2) as u32;
  if rt != 31 {
    // Zero, zero in the lower half only, or random
    input.x[rt as usize] = match rng.below(4) {
      0 => 0,
      1 => rng.next() << 32,
      _ => rng.next(),
    };
  }
  (sf << 31 | 0x3400_0000 | op << 24 | (imm(pc, target, 19) << 5) | rt, None)
}

fn test_and_branch(rng: &mut Rng, input: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let target = target(rng, region, pc, 1 << 15, 4);
  let rt = rng.register();
  let bit = rng.below(64) as u32;
  let op = rng.below(2) as u32;
  if rt != 31 {
    let mask = 1 << bit;
    let value = &mut input.x[rt as usize];
    *value = if rng.bool() { *value | mask } else { *value & !mask };
  }
  let instruction = (bit >> 5) << 31 | 0x3600_0000 | op << 24 | (bit & 0x1F) << 19 | (imm(pc, target, 14) << 5) | rt;
  (instruction, None)
}

fn address(rng: &mut Rng, _: &mut Context, _: &Region, _: usize) -> (u32, Option<usize>) {
  let page = rng.below(2) as u32;
  let imm = rng.next() as u32;
  (page << 31 | (imm & 0x3) << 29 | 0x1000_0000 | (imm & 0x1F_FFFC) << 3 | rng.register(), None)
}

fn literal(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let opc = rng.below(4) as u32;
  let size = [4, 8, 4, 0][opc as usize];
  let target = target(rng, region, pc, 1 << 20, size.max(4));
  // PRFM accepts any operation
  let rt = if opc == 0b11 { rng.below(32) as u32 } else { rng.register() };
  (opc << 30 | 0x1800_0000 | (imm(pc, target, 19) << 5) | rt, None)
}

fn simd_literal(rng: &mut Rng, _: &mut Context, region: &Region, pc: usize) -> (u32, Option<usize>) {
  let opc = rng.below(3) as u32;
  let target = target(rng, region, pc, 1 << 20, [4, 8, 16][opc as usize]);
  let rt = rng.below(32) as u32;
  (opc << 30 | 0x1C00_0000 | (imm(pc, target, 19) << 5) | rt, None)
}

/// Executes code at `entry`, returning the resulting state.
fn execute(input: &Context, output: &mut Context, entry: usize) -> Context {
  let mut input = input.clone();
  input.entry = entry as u64;
  // SAFETY: The entry leads to the exit stub, which saves to `output`.
  unsafe { enter(&input, output) };
  output.clone()
}

/// Returns the differences between the state after the original (`expected`)
/// and the relocated code (`actual`).
fn differences(expected: &Context, actual: &Context) -> Vec<String> {
  let mut differences = Vec::new();
  let slot = |context: &Context| context.x[30].wrapping_sub(4);
  if slot(expected) != slot(actual) {
    differences.push(format!("slot: {:#x} != {:#x}", slot(expected), slot(actual)));
  }
  for index in REGISTERS.iter().filter(|&&index| index != 31).map(|&index| index as usize) {
    if expected.x[index] != actual.x[index] {
      differences.push(format!("x{index}: {:#x} != {:#x}", expected.x[index], actual.x[index]));
    }
  }
  for (index, (expected, actual)) in expected.q.iter().zip(&actual.q).enumerate() {
    if expected != actual {
      differences.push(format!("q{index}: {expected:#x} != {actual:#x}"));
    }
  }
  if expected.nzcv != actual.nzcv {
    differences.push(format!("nzcv: {:#x} != {:#x}", expected.nzcv, actual.nzcv));
  }
  if expected.sp != actual.sp {
    differences.push(format!("sp: {:#x} != {:#x}", expected.sp, actual.sp));
  }
  differences
}

/// Runs random instances of a class, relocated both within and beyond the
/// reach of a direct branch.
///
/// Returns the number of instances which fell through, and which branched.
fn check(name: &str, generate: Generator) -> [usize; 2] {
  let seed = env_or("DETOUR_RELOCATE_SEED", DEFAULT_SEED);
  let iterations = env_or("DETOUR_RELOCATE_ITERATIONS", DEFAULT_ITERATIONS as u64) as usize;
  let mut rng = Rng(seed ^ name.bytes().fold(0_u64, |hash, byte| hash.rotate_left(8) ^ u64::from(byte)));

  let mut output = Box::new(Context::default());
  let region = Region::map(0, REGION_LEN, &mut *output).expect("could not map a region");
  let far = Region::map_far(&region, FAR_DISTANCE, 0x1_0000, &mut *output);

  let mut outcomes = [0; 2];
  for iteration in 0..iterations {
    let mut input = Context {
      x: core::array::from_fn(|_| rng.next()),
      q: core::array::from_fn(|_| u128::from(rng.next()) << 64 | u128::from(rng.next())),
      nzcv: rng.below(16) << 28,
      ..Context::default()
    };

    // The last slot is excluded, since it precedes the relocated code
    let pc = region.base + SLOTS.start + rng.below(((SLOTS.end - SLOTS.start) / 4 - 1) as u64) as usize * 4;
    let (instruction, call) = generate(&mut rng, &mut input, &region, pc);

    if let Some(destination) = call {
      region.write(destination, &[RET]);
    }
    region.write(pc, &[instruction]);
    let expected = execute(&input, &mut output, pc);
    outcomes[usize::from(expected.x[30] != (pc + 8) as u64)] += 1;

    for (placement, base) in [(&region, region.base + NEAR), (&far, far.base + 0x1000)] {
      let mut emitter = Emitter::new(base);
      emitter.relocate(instruction, pc);
      emitter.jump(pc + 4);
      let code = emitter.instructions().to_vec();
      placement.write(base, &code);
      let actual = execute(&input, &mut output, base);
      placement.reset(base, code.len());

      let differences = differences(&expected, &actual);
      assert!(
        differences.is_empty(),
        "{name} #{iteration} (seed {seed:#x}): {:#010x} at {:#x}, relocated to {base:#x} as {:x?}:\n{}",
        instruction,
        pc,
        code,
        differences.join("\n"),
      );
    }

    region.reset(pc, 1);
    if let Some(destination) = call {
      region.reset(destination, 1);
    }
  }
  outcomes
}

/// Runs a conditional class, asserting that both outcomes are exercised.
fn check_conditional(name: &str, generate: Generator) {
  let [fallthrough, branched] = check(name, generate);
  assert!(fallthrough > 0 && branched > 0, "{name}: {fallthrough} fell through, {branched} branched");
}

#[test]
fn relocates_branch() {
  check("b", branch);
}

#[test]
fn relocates_branch_with_link() {
  check("bl", branch_with_link);
}

#[test]
fn relocates_conditional_branch() {
  check_conditional("b.cond", conditional_branch);
}

#[test]
fn relocates_compare_and_branch() {
  check_conditional("cbz", compare_and_branch);
}

#[test]
fn relocates_test_and_branch() {
  check_conditional("tbz", test_and_branch);
}

#[test]
fn relocates_address_calculations() {
  check("adr", address);
}

#[test]
fn relocates_literal_loads() {
  check("ldr", literal);
}

#[test]
fn relocates_simd_literal_loads() {
  check("ldr simd", simd_literal);
}
