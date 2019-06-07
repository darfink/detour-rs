// https://github.com/jhector/armhook-core/blob/master/Hook.cpp

#[repr(packed)]
struct CallAbs {
  // mov r0, <address>
  opcode0: u8,
  opcode1: u8,
  dummy0: u32,
  // blx r0
  address: usize,
}
