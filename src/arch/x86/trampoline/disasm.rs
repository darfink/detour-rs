//! The underlying disassembler should be opaque to the outside.
use std::slice;

/// A x86/x64 disassembler.
pub struct Disassembler(udis::ud);

impl Disassembler {
  /// Creates a default x86 disassembler.
  pub fn new(target: *const ()) -> Disassembler {
    unsafe {
      let mut ud = ::std::mem::zeroed();
      udis::ud_init(&mut ud);
      udis::ud_set_user_opaque_data(&mut ud, target as *mut _);
      udis::ud_set_input_hook(&mut ud, Some(Self::udis_read_address));
      udis::ud_set_mode(&mut ud, (::std::mem::size_of::<usize>() * 8) as u8);
      Disassembler(ud)
    }
  }

  /// Reads one byte from a pointer and advances it.
  unsafe extern "C" fn udis_read_address(ud: *mut udis::ud) -> libc::c_int {
    let pointer = udis::ud_get_user_opaque_data(ud) as *mut u8;
    let result = *pointer;
    udis::ud_set_user_opaque_data(ud, pointer.offset(1) as *mut _);
    libc::c_int::from(result)
  }
}

/// Safe wrapper around an instruction.
pub struct Instruction {
  address: usize,
  mnemonic: udis::ud_mnemonic_code,
  operands: Vec<udis::ud_operand>,
  bytes: &'static [u8],
}

impl Instruction {
  /// Disassembles a new instruction at the specified address.
  pub unsafe fn new(disasm: &mut Disassembler, address: *const ()) -> Option<Self> {
    let instruction_bytes = udis::ud_disassemble(&mut disasm.0) as usize;
    if instruction_bytes > 0 {
      Some(Instruction {
        address: address as usize,
        mnemonic: udis::ud_insn_mnemonic(&disasm.0),
        operands: disasm.0.operand.to_vec(),
        bytes: slice::from_raw_parts(address as *const _, instruction_bytes),
      })
    } else {
      None
    }
  }

  /// Returns the instruction's address.
  pub fn address(&self) -> usize {
    self.address
  }

  /// Returns the next instruction's address.
  pub fn next_instruction_address(&self) -> usize {
    self.address() + self.len()
  }

  /// Returns the instructions relative branch offset, if applicable.
  pub fn relative_branch_displacement(&self) -> Option<isize> {
    unsafe {
      self
        .operands
        .iter()
        .find(|op| op.otype == udis::ud_type::UD_OP_JIMM)
        .map(|op| match op.size {
          8 => op.lval.sbyte as isize,
          32 => op.lval.sdword as isize,
          _ => unreachable!("Operand size: {}", op.size),
        })
    }
  }

  /// Returns the instructions RIP operand displacement if applicable.
  pub fn rip_operand_displacement(&self) -> Option<isize> {
    unsafe {
      // The operands displacement (e.g `mov eax, [rip+0x10]` ⟶ 0x10)
      self
        .operands
        .iter()
        .find(|op| op.otype == udis::ud_type::UD_OP_MEM && op.base == udis::ud_type::UD_R_RIP)
        .map(|op| op.lval.sdword as isize)
    }
  }

  /// Returns true if this instruction any type of a loop.
  pub fn is_loop(&self) -> bool {
    match self.mnemonic {
      udis::ud_mnemonic_code::UD_Iloop
      | udis::ud_mnemonic_code::UD_Iloope
      | udis::ud_mnemonic_code::UD_Iloopne
      | udis::ud_mnemonic_code::UD_Ijecxz
      | udis::ud_mnemonic_code::UD_Ijcxz => true,
      _ => false,
    }
  }

  /// Returns true if this instruction is an unconditional jump.
  pub fn is_unconditional_jump(&self) -> bool {
    self.mnemonic == udis::ud_mnemonic_code::UD_Ijmp
  }

  /// Returns true if this instruction is a function call.
  pub fn is_call(&self) -> bool {
    self.mnemonic == udis::ud_mnemonic_code::UD_Icall
  }

  /// Returns true if this instruction is a return.
  pub fn is_return(&self) -> bool {
    self.mnemonic == udis::ud_mnemonic_code::UD_Iret
  }

  /// Returns the instruction's bytes.
  pub unsafe fn as_slice(&self) -> &[u8] {
    self.bytes
  }

  /// Returns the size of the instruction in bytes.
  pub fn len(&self) -> usize {
    self.bytes.len()
  }
}
