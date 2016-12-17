use std::mem;
use generic_array::{GenericArray, typenum};
use pic::{Thunkable, StaticThunk};

#[repr(packed)]
pub struct JumpShort {
    opcode: u8,
    operand: i8,
}

#[repr(packed)]
pub struct JumpRel {
    opcode: u8,
    operand: u32,
}

#[repr(packed)]
pub struct JccRel {
    opcode0: u8,
    opcode1: u8,
    operand: u32,
}

/// Constructs either a relative jump or call.
fn relative32(address: u32, is_jump: bool) -> Box<Thunkable> {
    Box::new(StaticThunk::<typenum::U5>::new(move |offset| {
        let code = JumpRel {
            opcode: if is_jump { 0xE9 } else { 0xE8 },
            operand: address.wrapping_sub((offset + mem::size_of::<JumpRel>()) as u32),
        };

        let slice: [u8; 5] = unsafe { mem::transmute(code) };
        GenericArray::from_slice(&slice)
    }))
}

/// Constructs a relative call operation.
pub fn call_rel32(address: u32) -> Box<Thunkable> {
    relative32(address, false)
}

/// Constructs a relative jump operation.
pub fn jmp_rel32(address: u32) -> Box<Thunkable> {
    relative32(address, true)
}

/// Constructs a conditional relative jump operation.
pub fn jcc_rel32(address: u32, condition: u8) -> Box<Thunkable> {
    Box::new(StaticThunk::<typenum::U6>::new(move |offset| {
        let code = JccRel {
            opcode0: 0x0F,
            opcode1: 0x80 | condition,
            operand: address.wrapping_sub((offset + mem::size_of::<JccRel>()) as u32),
        };

        let slice: [u8; 6] = unsafe { mem::transmute(code) };
        GenericArray::from_slice(&slice)
    }))
}

/// Constructs a relative short jump.
pub fn jmp_rel8(displacement: i8) -> Box<Thunkable> {
    Box::new(StaticThunk::<typenum::U2>::new(move |_| {
        let code = JumpShort {
            opcode: 0xEB,
            operand: displacement - mem::size_of::<JumpShort>() as i8,
        };

        let slice: [u8; 2] = unsafe { mem::transmute(code) };
        GenericArray::from_slice(&slice)
    }))
}
