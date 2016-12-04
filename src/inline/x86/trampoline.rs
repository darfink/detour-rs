use std::{slice, mem};

use libc;

use error::*;
use inline::pic;
use super::{udis, thunk};

pub struct Trampoline {
    generator: pic::Generator,
    prolog_size: usize,
}

// TODO: add support for hot patch below function if ret was found (CGunTarget::ObjectCaps)
// TODO: use memory pool
impl Trampoline {
    /// Constructs a new trampoline for the specified function.
    pub unsafe fn new(target: *const(), margin: usize) -> Result<Trampoline> {
        let (generator, prolog_size) = TrampolineGen::process(udis_create(target), target, margin)?;
        Ok(Trampoline {
            prolog_size: prolog_size,
            generator: generator,
        })
    }

    /// Returns a reference to the trampoline's generator.
    pub fn generator(&self) -> &pic::Generator {
        &self.generator
    }

    /// Returns the size of the prolog (i.e the amount of disassembled bytes).
    pub fn prolog_size(&self) -> usize {
        self.prolog_size
    }
}

/// State describing the current instruction being processed.
#[derive(Debug)]
struct State {
    instruction: &'static [u8],
    mnemonic: udis::ud_mnemonic_code,
}

/// A trampoline generator (x86/x64).
struct TrampolineGen {
    disassembler: udis::ud,
    total_bytes_disassembled: usize,
    jump_address: Option<usize>,
    finished: bool,
    target: *const (),
    margin: usize,
}

// TODO: should larger margins be accounted for?
impl TrampolineGen {
    /// Processes a function until `margin` bytes have been disassembled.
    pub unsafe fn process(disassembler: udis::ud, target: *const(), margin: usize) -> Result<(pic::Generator, usize)> {
        TrampolineGen {
            disassembler: disassembler,
            total_bytes_disassembled: 0,
            jump_address: None,
            finished: false,
            target: target,
            margin: margin,
        }.process_impl()
    }

    /// Internal implementation for the `process` function.
    pub unsafe fn process_impl(mut self) -> Result<(pic::Generator, usize)> {
        let mut generator = pic::Generator::new();

        while !self.finished {
            let state = self.next_instruction()?;
            let thunk = self.process_instruction(&state)?;

            // If the trampoline displacement is larger than the target function,
            // all instructions will be offset, and if there is internal branching,
            // it will end up at the wrong instructions.
            if (state.instruction.as_ptr() as usize) < self.jump_address.unwrap_or(0) &&
                state.instruction.len() != thunk.len() {
                bail!(ErrorKind::UnsupportedRelativeBranch);
            } else {
                generator.add_thunk(thunk);
            }

            // Determine whether enough bytes for the margin has been disassembled
            if self.total_bytes_disassembled >= self.margin && !self.finished {
                // The entire prolog is available - determine the next instruction
                let next_instruction_address = state.instruction
                    .as_ptr().offset(state.instruction.len() as isize);

                // Add a jump to the first instruction after the prolog
                generator.add_thunk(thunk::jmp(mem::transmute(next_instruction_address)));
                self.finished = true;
            }
        }

        Ok((generator, self.total_bytes_disassembled))
    }

    /// Disassembles the next instruction and returns the current state.
    unsafe fn next_instruction(&mut self) -> Result<State> {
        let instruction_bytes = udis::ud_disassemble(&mut self.disassembler) as usize;
        if instruction_bytes == 0 {
            // Since the source is a pointer, zero disassembled bytes indicate
            // that it contains invalid instructions.
            Err(ErrorKind::InvalidCode.into())
        } else {
            let state = State {
                mnemonic: udis::ud_insn_mnemonic(&self.disassembler),
                instruction: slice::from_raw_parts(
                    (self.target as usize + self.total_bytes_disassembled) as *const u8,
                    instruction_bytes),
            };

            // Keep track of the total amount of bytes
            self.total_bytes_disassembled += instruction_bytes;
            Ok(state)
        }
    }

    /// Analyses and modifies an instruction if required.
    unsafe fn process_instruction(&mut self, state: &State) -> Result<Box<pic::Thunkable>> {
        if let Some(relop) = Self::find_rip_relative_operand(&self.disassembler.operand) {
            self.handle_rip_relative_instruction(state, relop)
        } else if let Some(relop) = Self::find_branch_relative_operand(&self.disassembler.operand) {
            self.handle_relative_branch(state, relop)
        } else {
            if Self::is_return(state.mnemonic) {
                // TODO: move this repetetive check to a helper
                // In case the operand is not placed in a branch, the function
                // returns unconditionally, which means that it terminates here.
                self.finished = self.jump_address.map_or(true, |offset| state.instruction.as_ptr() as usize >= offset);
            }

            // The instruction does not use any position-dependant operands,
            // therefore the bytes can be copied directly from source.
            Ok(Box::new(state.instruction.to_vec()))
        }
    }

    /// Adjusts the offsets for RIP relative operands. They are only available
    /// in x64 processes. The operands offsets needs to be adjusted for their
    /// new position. An example would be:
    ///
    /// ```asm
    /// mov eax, [rip+0x10]   ; the displacement before relocation
    /// mov eax, [rip+0x4892] ; a theoretical adjustment after relocation
    /// ```
    unsafe fn handle_rip_relative_instruction(&mut self,
                                              state: &State,
                                              relative_operand: udis::ud_operand)
                                              -> Result<Box<pic::Thunkable>> {
        // If the instruction is an absolute indirect jump, processing stops here
        self.finished = Self::is_jmp(state.mnemonic);

        // These need to be captured by the closure
        let instruction_address = state.instruction.as_ptr() as usize;
        let instruction_bytes = state.instruction.to_vec();
        let instruction_size = state.instruction.len();

        Ok(Box::new(pic::Dynamic::new(move |offset| {
            let mut bytes = instruction_bytes.clone();

            // The operands displacement (e.g `mov eax, [rip+0x10]` == 0x10)
            let displacement = relative_operand.lval.udword as usize;

            // Calculate the new relative displacement for the operand
            let adjusted_displacement = instruction_address
                .wrapping_add(displacement)
                .wrapping_sub(offset) as u32;
            let as_bytes: [u8; 4] = mem::transmute(adjusted_displacement);

            // The displacement value is placed at (instruction - immediate value length - 4)
            let index = instruction_size - relative_operand.size as usize - mem::size_of::<u32>();

            // Write the adjusted displacement offset to the operand
            bytes[index..instruction_size].copy_from_slice(&as_bytes);
            bytes
        }, instruction_size)))
    }

    /// Processes relative branches (e.g `call`, `loop`, `jne`).
    unsafe fn handle_relative_branch(&mut self,
                                     state: &State,
                                     relative_operand: udis::ud_operand)
                                     -> Result<Box<pic::Thunkable>> {
        // Acquire the immediate value from the operand
        let relative_offset = match relative_operand.size {
            8 => relative_operand.lval.ubyte as usize,
            _ => relative_operand.lval.udword as usize,
        };

        // Calculate the absolute address of the target destination
        let destination_address_abs = state.instruction.as_ptr().offset(state.instruction.len() as isize) as usize
                                    + relative_offset;

        if state.mnemonic == udis::ud_mnemonic_code::UD_Icall {
            // Calls are a non-issue since they return to the original address
            return Ok(thunk::call(mem::transmute(destination_address_abs)));
        }

        let prolog_range = (self.target as usize)..(self.target as usize + self.margin);

        // If the relative jump is internal, and short enough to
        // fit within the copied function prolog (i.e `margin`),
        // the jump bytes can be copied indiscriminately.
        if prolog_range.contains(destination_address_abs) {
            // Keep track of the jump's destination address
            self.jump_address = Some(destination_address_abs);
            Ok(Box::new(state.instruction.to_vec()))
        } else if state.instruction[0] & 0xFC == 0xE0 {
            // Loops (e.g 'loopnz', 'jecxz') to the outside are not supported
            Err(ErrorKind::ExternalLoop.into())
        } else if Self::is_jmp(state.mnemonic) {
            // If the function is not in a branch, and it unconditionally jumps
            // a distance larger than the prolog, it's the same as if it terminates.
            self.finished = self.jump_address.map_or(true, |offset| state.instruction.as_ptr() as usize >= offset);
            Ok(thunk::jmp(mem::transmute(destination_address_abs)))
        } else /* Conditional jumps (Jcc) */ {
            // To extract the condition, the primary opcode is required. Short
            // jumps are only one byte, but long jccs are prefixed with 0x0F.
            let primary_opcode = state.instruction.iter().find(|op| **op != 0x0F).unwrap();

            // Extract the condition (i.e 0x74 is [jz rel8] -> 0x74 & 0x0F == 4)
            let condition = primary_opcode & 0x0F;
            Ok(thunk::jcc(mem::transmute(destination_address_abs), condition))
        }
    }

    /// Returns the instructions relative branch operand if found.
    fn find_branch_relative_operand(operands: &[udis::ud_operand]) -> Option<udis::ud_operand> {
        operands.iter().find(|op| op.otype == udis::ud_type::UD_OP_JIMM).map(|op| *op)
    }

    /// Returns the instructions RIP relative operand if found.
    fn find_rip_relative_operand(operands: &[udis::ud_operand]) -> Option<udis::ud_operand> {
        operands.iter().find(|op| {
            op.otype == udis::ud_type::UD_OP_MEM && op.base == udis::ud_type::UD_R_RIP
        }).map(|op| *op)
    }

    /// Returns true if the opcode is `jmp`.
    fn is_jmp(mnemonic: udis::ud_mnemonic_code) -> bool {
        mnemonic == udis::ud_mnemonic_code::UD_Ijmp
    }

    /// Returns true if the opcode is `ret` or `retn`.
    fn is_return(mnemonic: udis::ud_mnemonic_code) -> bool {
        mnemonic == udis::ud_mnemonic_code::UD_Iret
    }
}

/// Creates a default x86 disassembler
unsafe fn udis_create(target: *const ()) -> udis::ud {
    let mut ud = mem::zeroed();
    udis::ud_init(&mut ud);
    udis::ud_set_user_opaque_data(&mut ud, target as *mut _);
    udis::ud_set_input_hook(&mut ud, Some(udis_read_address));
    udis::ud_set_mode(&mut ud, (mem::size_of::<usize>() * 8) as u8);
    ud
}

/// Reads one byte from a pointer an advances it one byte.
unsafe extern "C" fn udis_read_address(ud: *mut udis::ud) -> libc::c_int {
    let pointer = udis::ud_get_user_opaque_data(ud) as *mut u8;
    let result = *pointer;
    udis::ud_set_user_opaque_data(ud, pointer.offset(1) as *mut _);
    result as _
}

