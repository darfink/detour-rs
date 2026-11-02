//! Checks the relocator's classification against a reference decoder.
//!
//! Every instruction `raki` decodes with a PC-relative operand must be
//! rewritten when relocated beyond its reach, and every instruction the
//! relocator rewrites must be PC-relative. The decoder also checks the
//! instruction length, terminators, branch targets and T1 usage.
//!
//! `raki` decodes RV64IMAC (with `Zicsr`, `Zifencei`, `Zicfiss` & privileged
//! instructions), which contains every PC-relative instruction of RV64GC;
//! F & D instructions are reported as invalid.

use super::encode::{self, Emitter};
use raki::{BaseIOpcode, COpcode, Decode, Instruction, Isa, OpcodeKind};
use std::collections::BTreeMap;
use std::format;
use std::string::String;
use std::vec::Vec;

const PC: usize = 0x1_0000_0000;
const FAR: usize = 0x7_0000_0000;

/// The outcome of classifying a single instruction.
enum Mismatch {
  /// The reference decoder reports a PC-relative operand, but the
  /// instruction is copied verbatim.
  Copied,
  /// The instruction is rewritten, but the reference decoder reports no
  /// PC-relative operand (or rejects it).
  Rewritten,
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
}

/// Decodes `word` (a halfword if compressed), or returns `None` if it is
/// invalid (or longer than 4 bytes).
fn decode(word: u32) -> Option<Instruction> {
  if encode::is_compressed(word) {
    (word as u16).decode(Isa::Rv64).ok()
  } else if word & 0b1_1100 != 0b1_1100 {
    word.decode(Isa::Rv64).ok()
  } else {
    None
  }
}

fn mnemonic(instruction: &Instruction) -> String {
  let text = format!("{instruction}");
  text.split_whitespace().next().unwrap_or_default().into()
}

/// Returns whether `instruction` has a PC-relative operand.
///
/// `AUIPC X0` is excluded: it has no effect (and is `LPAD` with `Zicfilp`).
fn is_pc_relative(instruction: &Instruction) -> bool {
  match instruction.opc {
    OpcodeKind::BaseI(BaseIOpcode::AUIPC) => instruction.rd != Some(0),
    OpcodeKind::BaseI(
      BaseIOpcode::JAL
      | BaseIOpcode::BEQ
      | BaseIOpcode::BNE
      | BaseIOpcode::BLT
      | BaseIOpcode::BGE
      | BaseIOpcode::BLTU
      | BaseIOpcode::BGEU,
    ) => true,
    OpcodeKind::C(COpcode::J | COpcode::JAL | COpcode::BEQZ | COpcode::BNEZ) => true,
    _ => false,
  }
}

/// Returns the bytes of `word` (2 or 4).
fn bytes(word: u32) -> Vec<u8> {
  let len = if encode::is_compressed(word) { 2 } else { 4 };
  word.to_le_bytes()[..len].to_vec()
}

/// Returns whether `word` is rewritten when relocated out of reach.
fn is_rewritten(word: u32) -> bool {
  let mut emitter = Emitter::new(FAR);
  emitter.relocate(word, PC);
  emitter.bytes() != bytes(word)
}

/// Classifies `word`, returning its mnemonic on a mismatch.
fn classify(word: u32) -> Option<(Mismatch, String)> {
  let decoded = decode(word);
  let is_pc_relative = decoded.as_ref().is_some_and(is_pc_relative);
  let mnemonic = || decoded.as_ref().map_or_else(|| "<invalid>".into(), mnemonic);

  match (is_pc_relative, is_rewritten(word)) {
    (true, false) => Some((Mismatch::Copied, mnemonic())),
    (false, true) => Some((Mismatch::Rewritten, mnemonic())),
    _ => None,
  }
}

/// Random words, every compressed instruction, and every 32-bit major opcode
/// with random operands.
fn words() -> impl Iterator<Item = u32> {
  let mut rng = Rng(0x0D37_0A64);
  let random = (0..0x4_0000).map(move |_| rng.next() as u32);
  let compressed = (0..0x1_0000_u32).filter(|&word| encode::is_compressed(word));
  let mut rng = Rng(0xA64_0D37);
  let targeted = (0..0x80_u32 * 0x800)
    .map(move |i| (rng.next() as u32 & !0x7F) | (i >> 11) | 0b11)
    .filter(|&word| word & 0b1_1100 != 0b1_1100);
  random
    .map(|word| if encode::is_compressed(word) { word & 0xFFFF } else { word })
    .filter(|&word| encode::length(word as u16).is_some())
    .chain(compressed)
    .chain(targeted)
}

/// Mismatches, grouped by mnemonic, with an example encoding and a count.
type Report = BTreeMap<String, (u32, usize)>;

fn mismatches() -> (Report, Report) {
  let mut copied = Report::new();
  let mut rewritten = Report::new();

  for word in words() {
    if let Some((kind, mnemonic)) = classify(word) {
      let report = match kind {
        Mismatch::Copied => &mut copied,
        Mismatch::Rewritten => &mut rewritten,
      };
      report.entry(mnemonic).or_insert((word, 0)).1 += 1;
    }
  }

  std::eprintln!("copied PC-relative instructions: {copied:#x?}");
  std::eprintln!("rewritten non-PC-relative instructions: {rewritten:#x?}");
  (copied, rewritten)
}

#[test]
fn rewrites_pc_relative_instructions() {
  let (copied, _) = mismatches();
  assert!(copied.is_empty(), "PC-relative instructions copied verbatim: {copied:#x?}");
}

#[test]
fn rewrites_only_pc_relative_instructions() {
  let (_, rewritten) = mismatches();
  // Unallocated encodings within a handled class may be rewritten (e.g. a
  // conditional branch with `funct3 = 0b010`)
  let valid: Vec<_> = rewritten
    .iter()
    .filter(|(mnemonic, _)| *mnemonic != "<invalid>")
    .collect();
  assert!(valid.is_empty(), "non-PC-relative instructions rewritten: {valid:#x?}");
}

#[test]
fn agrees_on_lengths() {
  for word in words() {
    let Some(instruction) = decode(word) else {
      continue;
    };
    let expected = if instruction.is_compressed { 2 } else { 4 };
    assert_eq!(encode::length(word as u16), Some(expected), "{word:#010x} ({instruction})");
    assert_eq!(encode::is_compressed(word), instruction.is_compressed, "{word:#010x} ({instruction})");
  }
}

#[test]
fn agrees_on_terminators() {
  for word in words() {
    let Some(instruction) = decode(word) else {
      continue;
    };
    let expected = match instruction.opc {
      OpcodeKind::BaseI(BaseIOpcode::JAL | BaseIOpcode::JALR) => instruction.rd == Some(0),
      OpcodeKind::C(COpcode::J) => true,
      // `C.JR X0` is reserved, but decoded by `raki`
      OpcodeKind::C(COpcode::JR) => instruction.rs1 != Some(0),
      _ => false,
    };
    assert_eq!(encode::is_terminator(word), expected, "{word:#010x} ({instruction})");
  }
}

#[test]
fn agrees_on_branch_targets() {
  for word in words() {
    let Some(instruction) = decode(word) else {
      continue;
    };
    let expected = match instruction.opc {
      OpcodeKind::BaseI(BaseIOpcode::AUIPC) => None,
      _ if is_pc_relative(&instruction) => {
        let offset = instruction.imm.expect("a PC-relative offset");
        Some(PC.wrapping_add(offset as isize as usize))
      },
      _ => None,
    };
    assert_eq!(encode::branch_target(word, PC), expected, "{word:#010x} ({instruction})");
  }
}

#[test]
fn detects_t1_usage() {
  for word in words() {
    let Some(instruction) = decode(word) else {
      continue;
    };
    let uses_t1 = [instruction.rd, instruction.rs1, instruction.rs2].contains(&Some(encode::T1 as usize));
    if uses_t1 {
      assert!(encode::may_use_t1(word), "{word:#010x} ({instruction})");
    }
  }
}
