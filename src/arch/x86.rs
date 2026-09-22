//! x86 and x86-64 support, built upon the `iced-x86` disassembler and block
//! encoder.
//!
//! The target is redirected using a relative `jmp` (5 bytes). If the target is
//! too small, either trailing padding (`nop`/`int3`) or a hot-patch area
//! preceding the function is used. Detours further away than ±2 GiB (x86-64)
//! are reached via a relay, an absolute jump allocated near the target.

use super::Hook;
use crate::error::{Error, Result};
use crate::memory::{self, CodeBlock};
use iced_x86::{
  BlockEncoder, BlockEncoderOptions, Code, Decoder, DecoderOptions, FlowControl, Instruction,
  InstructionBlock, Mnemonic, OpKind,
};
use std::ops::Range;
use std::slice;

#[cfg(target_arch = "x86")]
const BITNESS: u32 = 32;
#[cfg(target_arch = "x86_64")]
const BITNESS: u32 = 64;

/// The maximum distance of generated code from its target.
///
/// Slightly less than 2 GiB, so any branch between the target's prolog and
/// its generated code is guaranteed to be within reach of a `rel32`.
#[cfg(target_arch = "x86_64")]
const MAX_DISTANCE: usize = 0x7FF0_0000;
/// All addresses are reachable using a `rel32`, since displacements wrap.
#[cfg(target_arch = "x86")]
const MAX_DISTANCE: usize = usize::MAX;

/// `jmp rel32`
const JMP_REL32_LEN: usize = 5;
/// `jmp rel8`
const JMP_REL8_LEN: usize = 2;
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

  let prolog = Prolog::decode(target, JMP_REL32_LEN)?;

  // Determine the area to patch with a jump to the detour
  let hot_patch =
    if prolog.len >= JMP_REL32_LEN || is_padding(target + prolog.len, JMP_REL32_LEN - prolog.len) {
      false
    } else if prolog.len >= JMP_REL8_LEN
      && is_padding(target.wrapping_sub(JMP_REL32_LEN), JMP_REL32_LEN)
      && memory::is_executable(target.wrapping_sub(JMP_REL32_LEN) as *const ())?
    {
      true
    } else {
      return Err(Error::NoPatchArea);
    };

  let patch_address = if hot_patch {
    target - JMP_REL32_LEN
  } else {
    target
  };
  let patch_len = JMP_REL32_LEN + if hot_patch { JMP_REL8_LEN } else { 0 };

  // Relocated references to the patched bytes would observe the detour jump
  if prolog.references_memory(patch_address..patch_address + patch_len) {
    return Err(Error::UnsupportedInstruction);
  }

  // A relay is required if the detour is out of reach for a `rel32`
  let jump_end = patch_address + JMP_REL32_LEN;
  let relay = if rel32(jump_end, detour).is_some() {
    None
  } else {
    let code = absolute_jump(detour);
    let mut relay = memory::allocate_near(patch_address, MAX_DISTANCE, code.len())?;
    // SAFETY: The relay has just been allocated, and cannot be executing.
    unsafe { relay.write(&code)? };
    Some(relay)
  };

  let destination = relay.as_ref().map_or(detour, CodeBlock::address);
  let displacement = rel32(jump_end, destination).ok_or(Error::OutOfMemory)?;

  let mut patched = Vec::with_capacity(patch_len);
  patched.push(0xE9);
  patched.extend_from_slice(&displacement.to_le_bytes());
  if hot_patch {
    // A short jump from the target to the hot-patch area preceding it
    patched.extend_from_slice(&[0xEB, (-(patch_len as i8)) as u8]);
  }

  // SAFETY: The patch area has been verified to be readable.
  let original = unsafe { slice::from_raw_parts(patch_address as *const u8, patch_len) }.to_vec();
  let trampoline = prolog.relocate(target)?;

  Ok(Hook {
    patch_address: patch_address as *mut u8,
    original,
    patched,
    trampoline,
    relay,
  })
}

/// The instructions of a target that are replaced by the detour jump.
struct Prolog {
  instructions: Vec<Instruction>,
  /// The number of bytes spanned by the instructions.
  len: usize,
  /// Whether the target returns, or unconditionally leaves, within the prolog.
  terminated: bool,
}

impl Prolog {
  /// Decodes instructions at `target` until at least `margin` bytes are
  /// covered, or the function unconditionally terminates.
  fn decode(target: usize, margin: usize) -> Result<Self> {
    const MAX_BYTES: usize = 64;

    let available = memory::readable_len(target, MAX_BYTES);
    // SAFETY: The range has been verified to be readable.
    let bytes = unsafe { slice::from_raw_parts(target as *const u8, available) };
    let mut decoder = Decoder::with_ip(BITNESS, bytes, target as u64, DecoderOptions::NONE);

    let margin_range = target as u64..(target + margin) as u64;
    // The furthest destination of a branch internal to the prolog
    let mut branch_end = target as u64;
    let mut instructions = Vec::new();
    let mut terminated = false;

    while decoder.position() < margin && !terminated {
      if !decoder.can_decode() {
        return Err(Error::InvalidCode);
      }

      let instruction = decoder.decode();
      if instruction.is_invalid() {
        return Err(Error::InvalidCode);
      }

      // Instructions preceding the destination of an internal branch are
      // conditionally executed, so the function has not yet terminated.
      let in_branch = instruction.ip() < branch_end;

      match instruction.flow_control() {
        FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch => {
          let destination = near_branch_target(&instruction);
          match destination {
            Some(destination) if margin_range.contains(&destination) => {
              branch_end = branch_end.max(destination);
            },
            _ if instruction.flow_control() == FlowControl::UnconditionalBranch => {
              terminated = !in_branch;
            },
            _ => {},
          }
        },
        FlowControl::Return | FlowControl::IndirectBranch => terminated = !in_branch,
        #[cfg(target_arch = "x86")]
        FlowControl::Call if instruction.code() == Code::Call_rel32_32 => {
          // Position-independent i686 code retrieves its own address using a
          // call (e.g. `call __x86.get_pc_thunk.bx`). The return address must
          // therefore refer to the original code, not the trampoline. Since a
          // `call rel32` is five bytes, the return address is always past the
          // patched area.
          let mut push = Instruction::with1(Code::Pushd_imm32, instruction.next_ip32())
            .map_err(|_| Error::UnsupportedInstruction)?;
          push.set_ip(instruction.ip());
          let jump = Instruction::with_branch(Code::Jmp_rel32_32, instruction.near_branch_target())
            .map_err(|_| Error::UnsupportedInstruction)?;

          instructions.extend([push, jump]);
          terminated = true;
          continue;
        },
        _ => {},
      }

      instructions.push(instruction);
    }

    let len = decoder.position();

    // Internal branches must target instruction boundaries, so they can be
    // relocated along with their destination.
    let prolog_range = target as u64..(target + len) as u64;
    let is_valid = instructions.iter().all(|instruction| {
      near_branch_target(instruction)
        .filter(|destination| prolog_range.contains(destination))
        .is_none_or(|destination| instructions.iter().any(|other| other.ip() == destination))
    });

    if !is_valid {
      return Err(Error::UnsupportedInstruction);
    }

    Ok(Prolog {
      instructions,
      len,
      terminated,
    })
  }

  /// Returns whether any instruction references memory in `range`.
  fn references_memory(&self, range: Range<usize>) -> bool {
    self.instructions.iter().any(|instruction| {
      instruction.is_ip_rel_memory_operand()
        && (range.start as u64..range.end as u64).contains(&instruction.ip_rel_memory_address())
    })
  }

  /// Relocates the prolog to a trampoline allocated close to `target`.
  fn relocate(&self, target: usize) -> Result<CodeBlock> {
    let mut instructions = self.instructions.clone();

    if !self.terminated {
      // Resume execution after the prolog in the original function
      let resume = (target + self.len) as u64;
      #[cfg(target_arch = "x86")]
      let code = Code::Jmp_rel32_32;
      #[cfg(target_arch = "x86_64")]
      let code = Code::Jmp_rel32_64;

      // The block encoder redirects branches to addresses of instructions within
      // the block. The jump must therefore not be assigned the address it
      // branches to (its address is irrelevant, and defaults to zero).
      let jump =
        Instruction::with_branch(code, resume).map_err(|_| Error::UnsupportedInstruction)?;
      instructions.push(jump);
    }

    let capacity = self.len + instructions.len() * MAX_RELOCATED_LEN;
    let mut trampoline = memory::allocate_near(target, MAX_DISTANCE, capacity)?;

    // Relative branches and RIP-relative operands are adjusted for their new
    // location, and branches to relocated instructions are redirected.
    let block = InstructionBlock::new(&instructions, trampoline.address() as u64);
    let code = BlockEncoder::encode(BITNESS, block, BlockEncoderOptions::NONE)
      .map_err(|_| Error::UnsupportedInstruction)?
      .code_buffer;

    if code.len() > trampoline.len() {
      return Err(Error::UnsupportedInstruction);
    }

    // SAFETY: The trampoline has just been allocated, and cannot be executing.
    unsafe { trampoline.write(&code)? };
    Ok(trampoline)
  }
}

/// Returns the destination of a relative branch.
fn near_branch_target(instruction: &Instruction) -> Option<u64> {
  matches!(
    instruction.op0_kind(),
    OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
  )
  .then(|| instruction.near_branch_target())
}

/// Returns whether `len` bytes at `address` consist of code padding only.
fn is_padding(address: usize, len: usize) -> bool {
  // A multi-byte NOP may extend beyond the inspected range
  let available = memory::readable_len(address, len + 15);
  if available < len {
    return false;
  }

  // SAFETY: The range has been verified to be readable.
  let bytes = unsafe { slice::from_raw_parts(address as *const u8, available) };
  if bytes[..len].iter().all(|byte| matches!(byte, 0x90 | 0xCC)) {
    return true;
  }

  let mut decoder = Decoder::with_ip(BITNESS, bytes, address as u64, DecoderOptions::NONE);
  while decoder.position() < len {
    let instruction = decoder.decode();
    if instruction.is_invalid() || !matches!(instruction.mnemonic(), Mnemonic::Nop | Mnemonic::Int3)
    {
      return false;
    }
  }
  true
}

/// Returns the displacement of a relative jump ending at `source`, if
/// `destination` is within reach.
fn rel32(source: usize, destination: usize) -> Option<i32> {
  if cfg!(target_arch = "x86") {
    // Displacements wrap around the 32-bit address space
    Some(destination.wrapping_sub(source) as i32)
  } else {
    i32::try_from((destination as i64).wrapping_sub(source as i64)).ok()
  }
}

/// Returns an absolute, indirect jump (x86-64).
fn absolute_jump(destination: usize) -> Vec<u8> {
  // jmp qword ptr [rip+0]
  let mut code = vec![0xFF, 0x25, 0x00, 0x00, 0x00, 0x00];
  code.extend_from_slice(&(destination as u64).to_le_bytes());
  code
}
