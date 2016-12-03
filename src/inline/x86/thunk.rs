#![allow(dead_code)]
use generic_array::{GenericArray, ArrayLength};
use inline::pic;

#[cfg(target_arch = "x86_64")]
mod arch {
    pub use super::x64::jmp_abs as jmp;
    pub use super::x64::call_abs as call;
    pub use super::x64::jcc_abs as jcc;
}

#[cfg(target_arch = "x86")]
mod arch {
    pub use super::x86::jmp_rel32 as jmp;
    pub use super::x86::call_rel32 as call;
    pub use super::x86::jcc_rel32 as jcc;
}

// Export the assembly functions
pub use self::arch::*;

/// A closure that generates a thunk.
pub struct Thunk<N: ArrayLength<u8>>(Box<Fn(usize) -> GenericArray<u8, N>>);

impl<N: ArrayLength<u8>> Thunk<N> {
    /// Constructs a new thunk with a specific closure.
    fn new<T: Fn(usize) -> GenericArray<u8, N> + 'static>(callback: T) -> Self {
        Thunk(Box::new(callback))
    }
}

/// A static code generator for a thunk.
impl<N: ArrayLength<u8>> pic::Thunkable for Thunk<N> {
    fn generate(&self, address: usize) -> Vec<u8> {
        self.0(address).to_vec()
    }

    fn len(&self) -> usize {
        N::to_usize()
    }
}

/// Used for generating x86 assembly operations.
pub mod x86 {
    use std::mem;
    use generic_array::{GenericArray, typenum};
    use inline::pic::Thunkable;
    use super::*;

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
    struct JccRel {
        opcode0: u8,
        opcode1: u8,
        operand: u32,
    }

    /// Constructs either a relative jump or call.
    unsafe fn relative32(address: u32, is_jump: bool) -> Box<Thunkable> {
        Box::new(Thunk::<typenum::U5>::new(move |offset| {
            let code = JumpRel {
                opcode: if is_jump { 0xE9 } else { 0xE8 },
                operand: address.wrapping_sub((offset + mem::size_of::<JumpRel>()) as u32),
            };

            let slice: [u8; 5] = mem::transmute(code);
            GenericArray::from_slice(&slice)
        }))
    }

    /// Constructs a relative call operation.
    pub unsafe fn call_rel32(address: u32) -> Box<Thunkable> {
        relative32(address, false)
    }

    /// Constructs a relative jump operation.
    pub unsafe fn jmp_rel32(address: u32) -> Box<Thunkable> {
        relative32(address, true)
    }

    /// Constructs a conditional relative jump operation.
    pub unsafe fn jcc_rel32(address: u32, condition: u8) -> Box<Thunkable> {
        Box::new(Thunk::<typenum::U6>::new(move |offset| {
            let code = JccRel {
                opcode0: 0x0F,
                opcode1: 0x80 | condition,
                operand: address.wrapping_sub((offset + mem::size_of::<JccRel>()) as u32),
            };

            let slice: [u8; 6] = mem::transmute(code);
            GenericArray::from_slice(&slice)
        }))
    }

    /// Constructs a relative short jump.
    pub unsafe fn jmp_rel8(displacement: i8) -> Box<Thunkable> {
        Box::new(Thunk::<typenum::U2>::new(move |_| {
            let code = JumpShort {
                opcode: 0xEB,
                operand: displacement.wrapping_sub(mem::size_of::<JumpShort>() as i8),
            };

            let slice: [u8; 2] = mem::transmute(code);
            GenericArray::from_slice(&slice)
        }))
    }
}

/// Used for generating x86_64 assembly operations.
#[cfg(target_arch = "x86_64")]
pub mod x64 {
    use std::mem;
    use generic_array::{GenericArray, typenum};
    use inline::pic::Thunkable;
    use super::*;

    #[repr(packed)]
    struct CallAbs {
        // call [rip+8]
        opcode0: u8,
        opcode1: u8,
        dummy0: u32,
        // jmp +10
        dummy1: u8,
        dummy2: u8,
        // destination
        address: u64,
    }

    pub unsafe fn call_abs(address: u64) -> Box<Thunkable> {
        Box::new(Thunk::<typenum::U16>::new(move |_| {
            let code = CallAbs {
                opcode0: 0xFF,
                opcode1: 0x15,
                dummy0: 0x000000002,
                dummy1: 0xEB,
                dummy2: 0x08,
                address: address,
            };

            let slice: [u8; 16] = mem::transmute(code);
            GenericArray::from_slice(&slice)
        }))
    }

    #[repr(packed)]
    struct JumpAbs {
        // jmp +6
        opcode0: u8,
        opcode1: u8,
        dummy0: u32,
        // destination
        address: u64,
    }

    pub unsafe fn jmp_abs(address: u64) -> Box<Thunkable> {
        Box::new(Thunk::<typenum::U14>::new(move |_| {
            let code = JumpAbs {
                opcode0: 0xFF,
                opcode1: 0x25,
                dummy0: 0x000000000,
                address: address,
            };

            let slice: [u8; 14] = mem::transmute(code);
            GenericArray::from_slice(&slice)
        }))
    }

    #[repr(packed)]
    struct JccAbs {
        // jxx + 16
        opcode: u8,
        dummy0: u8,
        dummy1: u8,
        dummy2: u8,
        dummy3: u32,
        // destination
        address: u64,
    }

    pub unsafe fn jcc_abs(address: u64, condition: u8) -> Box<Thunkable> {
        Box::new(Thunk::<typenum::U16>::new(move |_| {
            let code = JccAbs {
                // Invert the condition in x64 mode to simplify the conditional jump logic
                opcode: 0x71 ^ condition,
                dummy0: 0x0E,
                dummy1: 0xFF,
                dummy2: 0x25,
                dummy3: 0x00000000,
                address: address,
            };

            let slice: [u8; 16] = mem::transmute(code);
            GenericArray::from_slice(&slice)
        }))
    }
}
