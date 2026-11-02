//! Checks the relocator's classification against a reference decoder.
//!
//! Every instruction `yaxpeax-arm` decodes with a PC-relative operand must be
//! rewritten when relocated beyond its reach, and every instruction the
//! relocator rewrites must be PC-relative.

use super::encode::Emitter;
use std::collections::BTreeMap;
use std::format;
use std::string::String;
use std::vec::Vec;
use yaxpeax_arch::{Decoder, U8Reader};
use yaxpeax_arm::armv8::a64::{InstDecoder, Operand};

const PC: usize = 0x1_0000_0000;
const FAR: usize = 0x7_0000_0000;

/// PC-relative instructions the relocator is known not to handle.
///
/// `FEAT_PAuth_LR` instructions use the address of a preceding `PAC*SPPC` as a
/// modifier, which differs once relocated.
const KNOWN_GAPS: &[&str] = &["autiasppc", "autibsppc", "retaasppc", "retabsppc"];

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

/// Returns the mnemonic of `word`, and whether it has a PC-relative operand,
/// or `None` if it fails to decode.
fn decode(decoder: &InstDecoder, word: u32) -> Option<(String, bool)> {
  let bytes = word.to_le_bytes();
  let instruction = decoder.decode(&mut U8Reader::new(&bytes)).ok()?;
  let is_pc_relative = instruction
    .operands
    .iter()
    .any(|operand| matches!(operand, Operand::PCOffset(_)));
  let text = format!("{instruction}");
  let mnemonic = text.split_whitespace().next().unwrap_or_default().into();
  Some((mnemonic, is_pc_relative))
}

/// Returns whether `word` is rewritten when relocated out of reach.
fn is_rewritten(word: u32) -> bool {
  let mut emitter = Emitter::new(FAR);
  emitter.relocate(word, PC);
  emitter.instructions() != [word]
}

/// Classifies `word`, returning its mnemonic on a mismatch.
fn classify(decoder: &InstDecoder, word: u32) -> Option<(Mismatch, String)> {
  let decoded = decode(decoder, word);
  let is_pc_relative = decoded.as_ref().is_some_and(|(_, pc_relative)| *pc_relative);
  let mnemonic = || {
    decoded
      .as_ref()
      .map_or_else(|| "<invalid>".into(), |(mnemonic, _)| mnemonic.clone())
  };

  match (is_pc_relative, is_rewritten(word)) {
    (true, false) => Some((Mismatch::Copied, mnemonic())),
    (false, true) => Some((Mismatch::Rewritten, mnemonic())),
    _ => None,
  }
}

/// Random words, and every top byte (which selects the classes the relocator
/// handles) with random operands.
fn words() -> impl Iterator<Item = u32> {
  let mut rng = Rng(0x0D37_0A64);
  let random = (0..0x4_0000).map(move |_| rng.next() as u32);
  let mut rng = Rng(0xA64_0D37);
  let targeted = (0..0x100_u32 * 0x400).map(move |i| ((i >> 10) << 24) | (rng.next() as u32 & 0xFF_FFFF));
  random.chain(targeted)
}

/// Mismatches, grouped by mnemonic, with an example encoding and a count.
type Report = BTreeMap<String, (u32, usize)>;

fn mismatches() -> (Report, Report) {
  let decoder = InstDecoder::default();
  let mut copied = Report::new();
  let mut rewritten = Report::new();

  for word in words() {
    if let Some((kind, mnemonic)) = classify(&decoder, word) {
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
  let unknown: Vec<_> = copied
    .iter()
    .filter(|(mnemonic, _)| !KNOWN_GAPS.contains(&mnemonic.as_str()))
    .collect();
  assert!(unknown.is_empty(), "PC-relative instructions copied verbatim: {unknown:#x?}");
}

#[test]
#[ignore = "relocator gap: FEAT_PAuth_LR instructions (e.g. `retaasppc`, 0x5500_001F) are copied verbatim"]
fn rewrites_pauth_lr_instructions() {
  let (copied, _) = mismatches();
  assert!(copied.is_empty(), "PC-relative instructions copied verbatim: {copied:#x?}");
}

#[test]
fn rewrites_only_pc_relative_instructions() {
  let (_, rewritten) = mismatches();
  // Unallocated encodings within a handled class may be rewritten (e.g. a
  // SIMD & FP literal load with `opc = 0b11`)
  let valid: Vec<_> = rewritten
    .iter()
    .filter(|(mnemonic, _)| *mnemonic != "<invalid>")
    .collect();
  assert!(valid.is_empty(), "non-PC-relative instructions rewritten: {valid:#x?}");
}
