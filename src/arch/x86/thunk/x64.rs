use crate::pic::Thunkable;
use std::mem;

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
  address: usize,
}

pub fn call_abs(destination: usize) -> Box<dyn Thunkable> {
  let code = CallAbs {
    opcode0: 0xFF,
    opcode1: 0x15,
    dummy0: 0x0_0000_0002,
    dummy1: 0xEB,
    dummy2: 0x08,
    address: destination,
  };

  let slice: [u8; 16] = unsafe { mem::transmute(code) };
  Box::new(slice.to_vec())
}

#[repr(packed)]
struct JumpAbs {
  // jmp +6
  opcode0: u8,
  opcode1: u8,
  dummy0: u32,
  // destination
  address: usize,
}

pub fn jmp_abs(destination: usize) -> Box<dyn Thunkable> {
  let code = JumpAbs {
    opcode0: 0xFF,
    opcode1: 0x25,
    dummy0: 0x0_0000_0000,
    address: destination,
  };

  let slice: [u8; 14] = unsafe { mem::transmute(code) };
  Box::new(slice.to_vec())
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
  address: usize,
}

pub fn jcc_abs(destination: usize, condition: u8) -> Box<dyn Thunkable> {
  let code = JccAbs {
    // Invert the condition in x64 mode to simplify the conditional jump logic
    opcode: 0x71 ^ condition,
    dummy0: 0x0E,
    dummy1: 0xFF,
    dummy2: 0x25,
    dummy3: 0x0000_0000,
    address: destination,
  };

  let slice: [u8; 16] = unsafe { mem::transmute(code) };
  Box::new(slice.to_vec())
}
