//! Differential tests of the relocator, executing random relative branches
//! and RIP-relative instructions in place and relocated, and comparing the
//! resulting state.
//!
//! Instructions run within a code region in which every 8-byte slot is
//! `CALL <stub>`. The stub saves all registers and returns to the harness, so
//! the slot reached (the return address pushed, minus 5) identifies where
//! execution continued.
//!
//! An instance is preceded by up to 3 `NOP`s, and padded up to the next slot
//! (and beyond the patch area), which is reached upon falling through. The
//! padding consists of `XCHG` & `CMC`, so resuming at the wrong offset is
//! observable. The prolog is decoded and relocated by [`Prolog`], the way a
//! detour is built, and its patch area is then overwritten with `CALL <stub>`
//! (reaching the entry's slot), as if it were patched.
//!
//! Trampolines are allocated from a pool pinned next to the region. On
//! x86-64, it is the only pool within reach, and each instance is also run in
//! a second region, as far from the pool as the relocator permits (so the
//! upper part of the region is beyond the reach of a `rel32` from the
//! trampoline).
//!
//! The code under test runs on a dedicated stack, which is compared as well.
//! On x86-64, a relocated `CALL` pushes the address of its successor in the
//! trampoline. The return address is therefore checked by resuming execution
//! at it instead.
//!
//! The seed and the number of iterations per class may be overridden with
//! `DETOUR_RELOCATE_SEED` and `DETOUR_RELOCATE_ITERATIONS`.

use super::{JMP_REL32_LEN, Prolog};
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock};
use core::arch::naked_asm;
use core::mem::offset_of;
use core::ops::Range;
use std::boxed::Box;
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

const DEFAULT_SEED: u64 = 0x0D37_0A64;
/// Fewer than on AArch64 & RISC-V, since each iteration rewrites code which
/// has executed (which is costly when translated, e.g. by Rosetta).
const DEFAULT_ITERATIONS: usize = 4_000;

/// The size of the region containing the original instructions.
const REGION_LEN: usize = 0x40_0000;
const SLOT_LEN: usize = 8;
/// The range of slots (relative to the region) for instructions & targets,
/// excluding [`DATA`].
const SLOTS: Range<usize> = 0x1000..REGION_LEN;
/// The range of slots for instructions, which are within `MAX_DISTANCE` of
/// the pool in the distant region.
const CODE: Range<usize> = 0x1000..0x8_0000;
/// The range of operands of RIP-relative instructions. From the pool, the
/// lower part is within reach in the distant region.
const DATA: Range<usize> = 0x10_0000..0x20_0000;
/// The size of the stack, in words.
const STACK_LEN: usize = 32;

/// The number of general-purpose registers.
#[cfg(target_arch = "x86_64")]
const REGISTERS: usize = 16;
#[cfg(target_arch = "x86")]
const REGISTERS: usize = 8;
const CX: usize = 1;
const SP: usize = 4;

/// The arithmetic flags (CF, PF, AF, ZF, SF & OF), which are randomized and
/// compared (see [`compared_flags`]).
const ARITHMETIC_FLAGS: usize = 0x8D5;
/// The reserved bit of `FLAGS`, which is always set.
const RESERVED_FLAGS: usize = 0x2;

const NOP: u8 = 0x90;
const INT3: u8 = 0xCC;
/// `XCHG EAX, r32` (other than ESP) and `CMC`, padding an instance by offset.
const PADDING: [u8; 7] = [0x91, 0x92, 0x93, 0x95, 0x96, 0x97, 0xF5];

/// The architectural state, loaded before and saved after execution.
#[repr(C)]
#[derive(Clone, Default)]
struct Context {
  /// The general-purpose registers, in encoding order (SP is set by the
  /// harness).
  r: [usize; REGISTERS],
  flags: usize,
  /// The address at which execution starts.
  entry: usize,
  /// The harness's stack pointer.
  host_sp: usize,
}

#[cfg(target_arch = "x86_64")]
const _: () = {
  assert!(offset_of!(Context, flags) == 128);
  assert!(offset_of!(Context, entry) == 136);
  assert!(offset_of!(Context, host_sp) == 144);
};

#[cfg(target_arch = "x86")]
const _: () = {
  assert!(offset_of!(Context, flags) == 32);
  assert!(offset_of!(Context, entry) == 36);
  assert!(offset_of!(Context, host_sp) == 40);
};

/// Loads `input` and branches to its entry, until the exit stub saves the
/// state to `output` (which the stub refers to) and returns.
///
/// The entry is pushed onto (and popped off) the stack under test, which is
/// therefore written below its stack pointer.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn enter(input: *const Context, output: *mut Context) {
  naked_asm!(
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rsi + 144], rsp",
    "push qword ptr [rdi + 128]",
    "popfq",
    "mov rax, [rdi + 0]",
    "mov rcx, [rdi + 8]",
    "mov rdx, [rdi + 16]",
    "mov rbx, [rdi + 24]",
    "mov rsp, [rdi + 32]",
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
    "push qword ptr [rdi + 136]",
    "mov rdi, [rdi + 56]",
    "ret",
  )
}

/// Saves the state to the context in RAX (with RAX saved by the stub), and
/// returns from [`enter`].
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn exit() {
  naked_asm!(
    "endbr64",
    "pushfq",
    "pop qword ptr [rax + 128]",
    // Clear the copy of the flags left on the stack under test, since they are
    // compared (and masked) separately
    "mov qword ptr [rsp - 8], 0",
    "mov [rax + 8], rcx",
    "mov [rax + 16], rdx",
    "mov [rax + 24], rbx",
    "mov [rax + 32], rsp",
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
    "mov rsp, [rax + 144]",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
  )
}

/// Loads `input` and branches to its entry, until the exit stub saves the
/// state to `output` (which the stub refers to) and returns.
///
/// The entry is pushed onto (and popped off) the stack under test, which is
/// therefore written below its stack pointer.
#[cfg(target_arch = "x86")]
#[unsafe(naked)]
unsafe extern "C" fn enter(input: *const Context, output: *mut Context) {
  naked_asm!(
    "push ebx",
    "push ebp",
    "push esi",
    "push edi",
    "mov eax, [esp + 20]",
    "mov edx, [esp + 24]",
    "mov [edx + 40], esp",
    "push dword ptr [eax + 32]",
    "popfd",
    "mov ecx, [eax + 4]",
    "mov edx, [eax + 8]",
    "mov ebx, [eax + 12]",
    "mov esp, [eax + 16]",
    "mov ebp, [eax + 20]",
    "mov esi, [eax + 24]",
    "mov edi, [eax + 28]",
    "push dword ptr [eax + 36]",
    "mov eax, [eax + 0]",
    "ret",
  )
}

/// Saves the state to the context in EAX (with EAX saved by the stub), and
/// returns from [`enter`].
#[cfg(target_arch = "x86")]
#[unsafe(naked)]
unsafe extern "C" fn exit() {
  naked_asm!(
    "endbr32",
    "pushfd",
    "pop dword ptr [eax + 32]",
    // Clear the copy of the flags left on the stack under test, since they are
    // compared (and masked) separately
    "mov dword ptr [esp - 4], 0",
    "mov [eax + 4], ecx",
    "mov [eax + 8], edx",
    "mov [eax + 12], ebx",
    "mov [eax + 16], esp",
    "mov [eax + 20], ebp",
    "mov [eax + 24], esi",
    "mov [eax + 28], edi",
    "mov esp, [eax + 40]",
    "pop edi",
    "pop esi",
    "pop ebp",
    "pop ebx",
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

  /// Returns a register, other than SP.
  fn register(&mut self) -> u8 {
    loop {
      let register = self.below(REGISTERS as u64) as u8;
      if usize::from(register) != SP {
        return register;
      }
    }
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

/// Returns the address at which to map a region.
///
/// On x86-64, each region resides at its own address, distant from any other
/// code, so the only pools within reach of it are those it pins.
fn hint() -> usize {
  #[cfg(target_arch = "x86_64")]
  {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    0x2000_0000_0000 + NEXT.fetch_add(1, Ordering::Relaxed) * 0x10_0000_0000
  }
  #[cfg(target_arch = "x86")]
  0
}

/// An executable mapping, filled with `CALL <stub>` slots (and data).
struct Region {
  base: usize,
  len: usize,
}

impl Region {
  /// Maps a region at `hint` (if available), with an exit stub that saves
  /// the state to `output`.
  fn map(hint: usize, len: usize, output: *mut Context) -> Option<Self> {
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
    };
    let rax = output.cast::<u8>().wrapping_add(offset_of!(Context, r)) as usize;
    let exit = exit as *const () as usize;
    #[cfg(target_arch = "x86_64")]
    // MOV [<output.rax>], RAX; MOV RAX, <output>; JMP [RIP]; .quad exit
    let stub = [
      &[0x48, 0xA3][..],
      &rax.to_le_bytes(),
      &[0x48, 0xB8],
      &(output as usize).to_le_bytes(),
      &[0xFF, 0x25, 0x00, 0x00, 0x00, 0x00],
      &exit.to_le_bytes(),
    ]
    .concat();
    #[cfg(target_arch = "x86")]
    // MOV [<output.eax>], EAX; MOV EAX, <output>; JMP exit
    let stub = [
      &[0xA3][..],
      &rax.to_le_bytes(),
      &[0xB8],
      &(output as usize).to_le_bytes(),
      &[0xE9],
      &exit.wrapping_sub(region.base + 15).to_le_bytes(),
    ]
    .concat();
    region.write(region.base, &stub);
    region.reset(region.base + SLOTS.start, region.base + len);
    Some(region)
  }

  fn contains(&self, address: usize) -> bool {
    (self.base..self.base + self.len).contains(&address)
  }

  /// Returns whether `address` is a slot available for instructions &
  /// targets.
  fn is_slot(&self, address: usize) -> bool {
    let offset = address.wrapping_sub(self.base);
    offset % SLOT_LEN == 0 && SLOTS.contains(&offset) && !DATA.contains(&offset)
  }

  /// Writes `code` at `address`.
  fn write(&self, address: usize, code: &[u8]) {
    assert!(self.contains(address) && self.contains(address + code.len() - 1));
    // SAFETY: The range is within the mapping, which is writable, and not
    // being executed.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u8, code.len()) };
    memory::flush_instruction_cache(address as *const u8, code.len());
  }

  /// Restores the slots spanning `start..end` (excluding the data).
  fn reset(&self, start: usize, end: usize) {
    let slots: Vec<u8> = (start..end)
      .step_by(SLOT_LEN)
      .flat_map(|slot| if self.is_slot(slot) { self.slot(slot) } else { [0; SLOT_LEN] })
      .collect();
    self.write(start, &slots);
  }

  /// Returns the code of the slot at `address`: `CALL <stub>; INT3 (x3)`.
  fn slot(&self, address: usize) -> [u8; SLOT_LEN] {
    let mut code = [INT3; SLOT_LEN];
    code[0] = 0xE8;
    code[1..5].copy_from_slice(&disp32(address + 5, self.base));
    code
  }
}

impl Drop for Region {
  fn drop(&mut self) {
    // SAFETY: The region is no longer referenced.
    unsafe { libc::munmap(self.base as *mut libc::c_void, self.len) };
  }
}

/// The location of an instance.
#[derive(Clone, Copy)]
struct Location {
  /// The slot at which execution starts (and which is relocated).
  entry: usize,
  /// The address of the instruction, following the leading `NOP`s.
  pc: usize,
}

/// The slots occupied by code at `entry` up to `end` (including its padding,
/// which spans the patch area); the end is the slot reached upon falling
/// through.
fn occupied(entry: usize, end: usize) -> Range<usize> {
  entry..end.max(entry + JMP_REL32_LEN).next_multiple_of(SLOT_LEN)
}

/// A random instance of an instruction class.
struct Instance {
  /// The code, following the leading `NOP`s.
  code: Vec<u8>,
  /// The address of a RIP-relative memory operand (8 bytes are compared).
  operand: Option<usize>,
  /// The destination of an indirect branch, stored at the operand.
  pointer: Option<usize>,
  /// Whether the instruction is a call, whose return address may refer to
  /// the trampoline.
  call: bool,
}

impl Instance {
  fn new(code: Vec<u8>) -> Self {
    Instance {
      code,
      operand: None,
      pointer: None,
      call: false,
    }
  }
}

/// Generates a random instance of an instruction class, located at `at`.
type Generator = fn(&mut Rng, &mut Context, &Region, Location) -> Instance;

/// Returns a random slot within `-range..range` bytes of `next` (the end of
/// a branch), avoiding the slots `occupied` (which would loop).
fn target(rng: &mut Rng, region: &Region, next: usize, range: usize, occupied: Range<usize>) -> usize {
  let low = (region.base + SLOTS.start).max(next.saturating_sub(range)).next_multiple_of(SLOT_LEN);
  let high = (region.base + region.len - SLOT_LEN).min(next + range - 1);
  loop {
    let target = low + rng.below(((high - low) / SLOT_LEN + 1) as u64) as usize * SLOT_LEN;
    if region.is_slot(target) && !occupied.contains(&target) {
      return target;
    }
  }
}

fn disp8(next: usize, target: usize) -> u8 {
  let displacement = target.wrapping_sub(next) as isize;
  assert!((-0x80..0x80).contains(&displacement), "{displacement:#x}");
  displacement as u8
}

fn disp32(next: usize, target: usize) -> [u8; 4] {
  let displacement = target.wrapping_sub(next) as isize;
  assert!(i32::try_from(displacement).is_ok(), "{displacement:#x}");
  (displacement as i32).to_le_bytes()
}

/// A short branch, with `opcode` (of 1 byte).
fn short(rng: &mut Rng, region: &Region, at: Location, opcode: u8) -> Instance {
  let next = at.pc + 2;
  let target = target(rng, region, next, 0x80, occupied(at.entry, next));
  Instance::new(vec![opcode, disp8(next, target)])
}

/// A near branch, with `opcode` (of 1 or 2 bytes).
fn near(rng: &mut Rng, region: &Region, at: Location, opcode: &[u8]) -> Instance {
  let next = at.pc + opcode.len() + 4;
  let target = target(rng, region, next, REGION_LEN, occupied(at.entry, next));
  Instance::new([opcode, &disp32(next, target)].concat())
}

fn short_jump(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  short(rng, region, at, 0xEB)
}

fn near_jump(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  near(rng, region, at, &[0xE9])
}

fn short_conditional_branch(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  let condition = rng.below(16) as u8;
  short(rng, region, at, 0x70 | condition)
}

fn near_conditional_branch(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  let condition = rng.below(16) as u8;
  near(rng, region, at, &[0x0F, 0x80 | condition])
}

/// `LOOPNE`, `LOOPE`, `LOOP` and `JRCXZ` (`JECXZ` on x86), optionally with an
/// address-size override (counting with ECX on x86-64, and CX on x86).
fn count_branch(rng: &mut Rng, input: &mut Context, region: &Region, at: Location) -> Instance {
  let opcode = 0xE0 + rng.below(4) as u8;
  let instance = if rng.bool() {
    let mut instance = short(rng, region, Location { pc: at.pc + 1, ..at }, opcode);
    instance.code.insert(0, 0x67);
    instance
  } else {
    short(rng, region, at, opcode)
  };
  // Zero or one, in the lower half only (with an override), or random
  input.r[CX] = match rng.below(4) {
    0 => 0,
    1 => 1,
    2 => (rng.next() as usize) << (usize::BITS / 2) | rng.below(2) as usize,
    _ => rng.next() as usize,
  };
  instance
}

fn call(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  Instance {
    call: true,
    ..near(rng, region, at, &[0xE8])
  }
}

/// A RIP-relative load, store, address calculation, arithmetic operation, or
/// indirect branch.
#[cfg(target_arch = "x86_64")]
fn rip_relative(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  let operand = region.base + DATA.start + rng.below((DATA.len() - 8) as u64) as usize;
  let register = rng.register();
  let rex = 0x48 | (register >> 3) << 2;
  let modrm = (register & 7) << 3 | 0b101;
  // The bytes preceding the displacement, and the size of the immediate
  let (prefix, immediate) = match rng.below(12) {
    // MOV r64, [RIP + disp]
    0 => (vec![rex, 0x8B, modrm], 0),
    // MOV r32, [RIP + disp]
    1 => (vec![0x40 | (register >> 3) << 2, 0x8B, modrm], 0),
    // LEA r64, [RIP + disp]
    2 => (vec![rex, 0x8D, modrm], 0),
    // CMP r64, [RIP + disp]
    3 => (vec![rex, 0x3B, modrm], 0),
    // ADD r64, [RIP + disp]
    4 => (vec![rex, 0x03, modrm], 0),
    // MOV [RIP + disp], r64
    5 => (vec![rex, 0x89, modrm], 0),
    // ADD [RIP + disp], r64
    6 => (vec![rex, 0x01, modrm], 0),
    // CMP qword ptr [RIP + disp], imm8
    7 => (vec![0x48, 0x83, 0x3D], 1),
    // MOV byte ptr [RIP + disp], imm8
    8 => (vec![0xC6, 0x05], 1),
    // PUSH qword ptr [RIP + disp]
    9 => (vec![0xFF, 0x35], 0),
    // JMP qword ptr [RIP + disp]
    10 => (vec![0xFF, 0x25], 0),
    // CALL qword ptr [RIP + disp]
    _ => (vec![0xFF, 0x15], 0),
  };
  let next = at.pc + prefix.len() + 4 + immediate;
  let indirect = prefix[..] == [0xFF, 0x25] || prefix[..] == [0xFF, 0x15];
  let call = prefix[..] == [0xFF, 0x15];
  let mut code = prefix;
  code.extend_from_slice(&disp32(next, operand));
  code.extend((0..immediate).map(|_| rng.next() as u8));
  Instance {
    code,
    operand: Some(operand),
    pointer: indirect.then(|| target(rng, region, next, REGION_LEN, occupied(at.entry, next))),
    call,
  }
}

/// A random register-to-register operation (setting the flags), modifying a
/// register other than SP.
fn arithmetic(rng: &mut Rng) -> Vec<u8> {
  let (source, destination) = (rng.below(REGISTERS as u64) as u8, rng.register());
  let mut code = Vec::new();
  #[cfg(target_arch = "x86_64")]
  {
    let rex = 0x40 | u8::from(rng.bool()) << 3 | (source >> 3) << 2 | destination >> 3;
    if rex != 0x40 || rng.bool() {
      code.push(rex);
    }
  }
  let modrm = |reg: u8| 0xC0 | (reg & 7) << 3 | destination & 7;
  // ADD, XOR, INC, or NOT
  code.extend_from_slice(&match rng.below(4) {
    0 => [0x01, modrm(source)],
    1 => [0x31, modrm(source)],
    2 => [0xFF, modrm(0)],
    _ => [0xF7, modrm(2)],
  });
  code
}

/// A short branch (usually conditional) skipping one or more of the
/// instructions following it, which are relocated along with it if they are
/// within the prolog (and are redirected to). One of them may jump out of the
/// prolog.
fn internal_branch(rng: &mut Rng, _: &mut Context, region: &Region, at: Location) -> Instance {
  let count = 1 + rng.below(3) as usize;
  let jump = (rng.below(4) == 0).then(|| rng.below(count as u64) as usize);
  let mut skipped: Vec<Vec<u8>> = (0..count)
    .map(|index| if Some(index) == jump { vec![0xEB, 0] } else { arithmetic(rng) })
    .collect();

  let start = at.pc + 2;
  // The boundaries following each skipped instruction
  let boundaries: Vec<usize> = skipped
    .iter()
    .scan(start, |end, code| {
      *end += code.len();
      Some(*end)
    })
    .collect();
  let occupied = occupied(at.entry, boundaries[count - 1]);
  if let Some(index) = jump {
    let next = boundaries[index];
    skipped[index][1] = disp8(next, target(rng, region, next, 0x80, occupied));
  }

  let destination = boundaries[rng.below(count as u64) as usize];
  let opcode = if rng.below(4) == 0 { 0xEB } else { 0x70 | rng.below(16) as u8 };
  let mut code = vec![opcode, disp8(start, destination)];
  code.extend(skipped.concat());
  Instance::new(code)
}

/// The state after execution.
#[derive(Clone)]
struct State {
  context: Context,
  stack: [usize; STACK_LEN],
  /// The 8 bytes at the operand of a RIP-relative instruction.
  operand: u64,
  /// The slot reached, if the stack pointer is within the stack.
  slot: Option<usize>,
}

/// Executes code, with the stack under test and the output of the stub.
struct Machine {
  output: Box<Context>,
  stack: Box<[usize; STACK_LEN]>,
}

impl Machine {
  /// The initial stack pointer, in the middle of the stack (which may be
  /// read or written above it).
  fn sp(&self) -> usize {
    self.stack.as_ptr() as usize + STACK_LEN / 2 * size_of::<usize>()
  }

  /// Executes code at `entry`, with `initial` as the initial state, returning
  /// the resulting state.
  fn execute(&mut self, region: &Region, initial: &State, operand: Option<usize>, entry: usize) -> State {
    let mut input = initial.context.clone();
    input.r[SP] = self.sp();
    input.entry = entry;
    *self.stack = initial.stack;
    if let Some(address) = operand {
      region.write(address, &initial.operand.to_le_bytes());
    }

    // SAFETY: The entry leads to the exit stub, which saves to `output`.
    unsafe { enter(&input, &mut *self.output) };

    let context = (*self.output).clone();
    let index = context.r[SP].wrapping_sub(self.stack.as_ptr() as usize) / size_of::<usize>();
    State {
      slot: self.stack.get(index).map(|address| address.wrapping_sub(5)),
      stack: *self.stack,
      operand: operand.map_or(0, |address| {
        // SAFETY: The operand is within the data of the region.
        unsafe { (address as *const u64).read_unaligned() }
      }),
      context,
    }
  }
}

/// Returns the flags compared: the arithmetic flags, except AF when translated
/// by Rosetta 2, which does not reliably preserve it (it may change across
/// instructions not affecting it, e.g. `JMP`).
fn compared_flags() -> usize {
  #[cfg(all(target_arch = "x86_64", target_vendor = "apple"))]
  {
    /// The auxiliary carry flag.
    const AUXILIARY_CARRY_FLAG: usize = 0x10;

    static TRANSLATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let translated = *TRANSLATED.get_or_init(|| {
      let mut value: libc::c_int = 0;
      let mut size = size_of::<libc::c_int>();
      // SAFETY: The output buffer is valid and its size is passed along.
      let result = unsafe {
        libc::sysctlbyname(
          c"sysctl.proc_translated".as_ptr(),
          (&raw mut value).cast(),
          &mut size,
          core::ptr::null_mut(),
          0,
        )
      };
      result == 0 && value == 1
    });
    if translated {
      return ARITHMETIC_FLAGS & !AUXILIARY_CARRY_FLAG;
    }
  }
  ARITHMETIC_FLAGS
}

/// Returns the differences between the state after the original (`expected`)
/// and the relocated code (`actual`), ignoring the stack word at `ignored`
/// (an x86-64 call's return address, which refers to the trampoline).
fn differences(expected: &State, actual: &State, ignored: Option<usize>) -> Vec<String> {
  let mut differences = Vec::new();
  if expected.slot != actual.slot {
    differences.push(format!("slot: {:x?} != {:x?}", expected.slot, actual.slot));
  }
  for (index, (expected, actual)) in expected.context.r.iter().zip(&actual.context.r).enumerate() {
    if expected != actual {
      differences.push(format!("r{index}: {expected:#x} != {actual:#x}"));
    }
  }
  let flags = |state: &State| state.context.flags & compared_flags();
  if flags(expected) != flags(actual) {
    differences.push(format!("flags: {:#x} != {:#x}", flags(expected), flags(actual)));
  }
  for (index, (expected, actual)) in expected.stack.iter().zip(&actual.stack).enumerate() {
    if expected != actual && Some(index) != ignored {
      differences.push(format!("stack[{index}]: {expected:#x} != {actual:#x}"));
    }
  }
  if expected.operand != actual.operand {
    differences.push(format!("operand: {:#x} != {:#x}", expected.operand, actual.operand));
  }
  differences
}

/// The maximum distance of a trampoline from the pool's pin.
const PIN_DISTANCE: usize = 0x1_0000;

/// Maps the regions in which instances run, and pins a pool next to the
/// first, from which trampolines are allocated (existing pools are preferred
/// by the allocator, and a pinned pool is never unmapped).
fn map(output: *mut Context) -> (Vec<Region>, CodeBlock) {
  let near = Region::map(hint(), REGION_LEN, output).expect("could not map a region");
  // On x86, every address is within reach (and the memory above the region
  // is typically in use, since mappings are allocated top-down)
  let distance = if cfg!(target_arch = "x86") { usize::MAX } else { PIN_DISTANCE };
  let pin = memory::allocate_near(near.base + near.len, distance, 1).expect("could not pin a pool");

  // The instructions of the distant region are within `MAX_DISTANCE` of the
  // pool, which reaches the lower part of its data only.
  #[cfg(target_arch = "x86_64")]
  let far = {
    let hint = pin.address() + super::MAX_DISTANCE - CODE.end - PIN_DISTANCE;
    Region::map(hint, REGION_LEN, output)
      .filter(|region| region.base == hint)
      .expect("could not map a distant region")
  };
  #[cfg(target_arch = "x86_64")]
  return (vec![near, far], pin);
  #[cfg(target_arch = "x86")]
  (vec![near], pin)
}

/// Relocates `prolog` at `entry` to a trampoline, which (on x86-64) must be
/// allocated from the pool of `pin`.
fn relocate(prolog: &Prolog, entry: usize, pin: &CodeBlock) -> Result<CodeBlock> {
  let _lock = memory::patch_lock();
  let (trampoline, _) = prolog.relocate(entry)?;
  assert!(
    cfg!(target_arch = "x86") || trampoline.address().abs_diff(pin.address()) < PIN_DISTANCE,
    "{trampoline:?} is not within the pool of {pin:?}"
  );
  Ok(trampoline)
}

/// Runs random instances of a class, relocated both close to and (on x86-64)
/// far from the instructions.
///
/// Returns the number of instances which fell through, and which branched.
fn check(name: &str, generate: Generator) -> [usize; 2] {
  let seed = env_or("DETOUR_RELOCATE_SEED", DEFAULT_SEED);
  let iterations = env_or("DETOUR_RELOCATE_ITERATIONS", DEFAULT_ITERATIONS as u64) as usize;
  let mut rng = Rng(seed ^ name.bytes().fold(0_u64, |hash, byte| hash.rotate_left(8) ^ u64::from(byte)));

  let mut machine = Machine {
    output: Box::new(Context::default()),
    stack: Box::new([0; STACK_LEN]),
  };
  let (regions, pin) = map(&mut *machine.output);

  let mut outcomes = [0; 2];
  for iteration in 0..iterations {
    let mut initial = State {
      context: Context {
        r: core::array::from_fn(|_| rng.next() as usize),
        flags: RESERVED_FLAGS | rng.next() as usize & ARITHMETIC_FLAGS,
        ..Context::default()
      },
      stack: core::array::from_fn(|_| rng.next() as usize),
      operand: rng.next(),
      slot: None,
    };

    // Generated for the first region; displacements are relative to it
    let offset = CODE.start + rng.below((CODE.len() / SLOT_LEN) as u64) as usize * SLOT_LEN;
    let at = Location {
      entry: regions[0].base + offset,
      pc: regions[0].base + offset + rng.below(4) as usize,
    };
    let Instance {
      code,
      operand,
      pointer,
      call,
    } = generate(&mut rng, &mut initial.context, &regions[0], at);

    for region in &regions {
      let entry = region.base + offset;
      let pc = at.pc - at.entry + entry;
      let operand = operand.map(|address| address - regions[0].base + region.base);
      let mut initial = initial.clone();
      if let Some(pointer) = pointer {
        initial.operand = (pointer - regions[0].base + region.base) as u64;
      }
      let slots = occupied(entry, pc + code.len());

      let mut bytes = vec![NOP; pc - entry];
      bytes.extend_from_slice(&code);
      bytes.extend((bytes.len()..slots.len()).map(|offset| PADDING[offset % PADDING.len()]));
      region.write(entry, &bytes);
      let expected = machine.execute(region, &initial, operand, entry);
      outcomes[usize::from(expected.slot != Some(slots.end))] += 1;

      let describe = |base: Option<usize>| {
        format!("{name} #{iteration} (seed {seed:#x}): {code:02x?} at {pc:#x}, relocated to {base:x?}")
      };
      let prolog = Prolog::decode(entry, JMP_REL32_LEN)
        .unwrap_or_else(|error| panic!("{}: decoding failed: {error:?}", describe(None)));
      // Branches into the patch area would observe the detour jump
      region.write(entry, &region.slot(entry)[..JMP_REL32_LEN]);

      let trampoline = match relocate(&prolog, entry, &pin) {
        Ok(trampoline) => trampoline,
        // RIP-relative operands beyond the reach of the trampoline are rejected
        Err(Error::UnsupportedInstruction)
          if operand.is_some_and(|operand| operand.abs_diff(pin.address()) > i32::MAX as usize - PIN_DISTANCE) =>
        {
          region.reset(slots.start, slots.end);
          continue;
        },
        Err(error) => panic!("{}: relocation failed: {error:?}", describe(None)),
      };
      let base = trampoline.address();
      let actual = machine.execute(region, &initial, operand, base);

      // The return address of an x86-64 call is checked by resuming at it
      let ignored = (call && cfg!(target_arch = "x86_64")).then_some(STACK_LEN / 2 - 1);
      let mut differences = differences(&expected, &actual, ignored);
      if let Some(index) = ignored {
        let (original, relocated) = (pc + code.len(), actual.stack[index]);
        if (base..base + trampoline.len()).contains(&relocated) {
          let expected = machine.execute(region, &initial, operand, original);
          let resumed = machine.execute(region, &initial, operand, relocated);
          differences.extend(
            self::differences(&expected, &resumed, None)
              .into_iter()
              .map(|difference| format!("resumed at {relocated:#x}: {difference}")),
          );
        } else {
          differences.push(format!("return address: {relocated:#x} is not within {trampoline:?}"));
        }
      }

      // SAFETY: The trampoline is not executing.
      let code = unsafe { core::slice::from_raw_parts(trampoline.as_ptr(), trampoline.len()) };
      assert!(
        differences.is_empty(),
        "{} as {code:02x?}:\n{}",
        describe(Some(base)),
        differences.join("\n"),
      );

      region.reset(slots.start, slots.end);
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
fn relocates_short_jumps() {
  check("jmp rel8", short_jump);
}

#[test]
fn relocates_near_jumps() {
  check("jmp rel32", near_jump);
}

#[test]
fn relocates_short_conditional_branches() {
  check_conditional("jcc rel8", short_conditional_branch);
}

#[test]
fn relocates_near_conditional_branches() {
  check_conditional("jcc rel32", near_conditional_branch);
}

#[test]
fn relocates_count_branches() {
  check_conditional("loop/jrcxz", count_branch);
}

#[test]
fn relocates_calls() {
  check("call rel32", call);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn relocates_rip_relative_operands() {
  check("rip-relative", rip_relative);
}

#[test]
fn relocates_internal_branches() {
  check_conditional("internal", internal_branch);
}
