use generic_array::{typenum, GenericArray};
use pic::{FixedThunk, Thunkable};
use std::mem;

// long branch with link (±4MB), pop after return, (6 bytes? 4 + 2)
// https://ece.uwaterloo.ca/~ece222/ARM/ARM7-TDMI-manual-pt3.pdf (5.19)
// http://infocenter.arm.com/help/topic/com.arm.doc.qrc0006e/QRC0006_UAL16.pdf
// https://github.com/Kingcom/armips/blob/440465fac0770a472580a6ae8ef0eb703d890d36/Archs/ARM/CThumbInstruction.cpp
// https://github.com/keystone-engine/keystone/blob/067d2bdfa34ea168b594d1967237db8cac619cb4/llvm/lib/Target/ARM/MCTargetDesc/ARMMCCodeEmitter.cpp
// https://github.com/ele7enxxh/Android-Inline-Hook/blob/master/relocate.c

// - thumb
// [nop]
// ldr.w pc, [pc, #0] (must be 4-byte aligned)
// .address

// - arm
// ldr pc, [pc, #-4]
// .address

#[packed]
struct Relay {
  pop_lr: u16,
  str_r0_lr: u16,
  ldr_r0_detour: u16,
  push_r0_detour: u16,
  ldr_r0_lr: u16,
  pop_pc: u16,
  data_detour: u32,
  data_cache: u32,
}

let is_both_thumb = ;
let is_both_arm = ;

if is_both_thumb && (-252..258).contains(offset) {
} else if is_both_arm && (-0x2000000..0x2000000).contains(offset) {
} else {
}

pub fn relay(destination: usize) -> Box<Thunkable> {
  let code = Relay {
    pop_lr: 0,
    str_r0_lr: 0,
    ldr_r0_detour: 0,
    push_r0_detour: 0,
    ldr_r0_lr: 0,
    pop_pc: 0,
    data_detour: 0,
    data_cache: 0,
  };

  let slice: [u8; 16] = unsafe { mem::transmute(code) };
  Box::new(slice.to_vec())
}

pub fn branch_with_link(destination: usize) -> Box<Thunkable> {
  // TODO: Validate target is thumb as well?
  Box::new(FixedThunk::<typenum::U4>::new(move |source| {
    let offset = encode_thumb_offset(source - destination - typenum::U4);

    let mut instruction = 0xF000D000;
    instruction |= (offset & 0x800000) << 3;
    instruction |= (offset & 0x1FF800) << 5;
    instruction |= (offset & 0x400000) >> 9;
    instruction |= (offset & 0x200000) >> 10;
    instruction |= offset & 0x7FF;

    let slice: [u8; 4] = unsafe { mem::transmute(instruction) };
    GenericArray::clone_from_slice(&slice)
  }))
}

// Thumb BL and BLX use a strange offset encoding where bits 22 and 21 are
// determined by negating them and XOR'ing them with bit 23.
fn encode_thumb_displacement(mut offset: u32) -> u32 {
  offset >>= 1;
  let sign = (offset & 0x800000) >> 23;
  let mut j1 = (offset & 0x400000) >> 22;
  let mut j2 = (offset & 0x200000) >> 21;
  j1 = !j1 & 0x1;
  j2 = !j2 & 0x1;
  j1 ^= sign;
  j2 ^= sign;

  offset &= !0x600000;
  offset |= j1 << 22;
  offset |= j2 << 21;

  return offset;
}
