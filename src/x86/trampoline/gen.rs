use std::{slice, mem};
use error::*;

use x86::{udis, thunk};
use pic;

/// A trampoline generator (x86/x64).
pub struct Generator {
    disassembler: udis::ud,
    total_bytes_disassembled: usize,
    branch_address: Option<usize>,
    finished: bool,
    target: *const (),
    margin: usize,
}

// TODO: should margins larger than 5 bytes be accounted for?
impl Generator {
    /// Processes a function until `margin` bytes have been disassembled.
    pub unsafe fn process(disassembler: udis::ud,
                          target: *const(),
                          margin: usize) -> Result<(pic::CodeBuilder, usize)> {
        Generator {
            disassembler: disassembler,
            total_bytes_disassembled: 0,
            branch_address: None,
            finished: false,
            target: target,
            margin: margin,
        }.process_impl()
    }

    /// Internal implementation for the `process` function.
    pub unsafe fn process_impl(mut self) -> Result<(pic::CodeBuilder, usize)> {
        let mut builder = pic::CodeBuilder::new();

        while !self.finished {
            let state = self.next_instruction()?;
            let thunk = self.process_instruction(&state)?;

            // If the trampoline displacement is larger than the target function,
            // all instructions will be offset, and if there is internal branching,
            // it will end up at the wrong instructions.
            if self.is_instruction_in_branch(&state) && state.instruction.len() != thunk.len() {
                bail!(ErrorKind::UnsupportedRelativeBranch);
            } else {
                builder.add_thunk(thunk);
            }

            // Determine whether enough bytes for the margin has been disassembled
            if self.total_bytes_disassembled >= self.margin && !self.finished {
                // Add a jump to the first instruction after the prolog
                builder.add_thunk(thunk::jmp(mem::transmute(state.next_instruction_address())));
                self.finished = true;
            }
        }

        Ok((builder, self.total_bytes_disassembled))
    }

    /// Disassembles the next instruction and returns the new state.
    unsafe fn next_instruction(&mut self) -> Result<State> {
        let instruction_bytes = udis::ud_disassemble(&mut self.disassembler) as usize;
        if instruction_bytes == 0 {
            // Since the source is a pointer, zero disassembled bytes indicate
            // that it contains invalid instructions.
            bail!(ErrorKind::InvalidCode);
        }

        let instruction_address = self.target as usize + self.total_bytes_disassembled;
        let state = State {
            mnemonic: udis::ud_insn_mnemonic(&self.disassembler),
            instruction: slice::from_raw_parts(instruction_address as *const _, instruction_bytes),
        };

        // Keep track of the total amount of bytes
        self.total_bytes_disassembled += instruction_bytes;
        Ok(state)
    }

    /// Analyses and modifies an instruction if required.
    unsafe fn process_instruction(&mut self, state: &State) -> Result<Box<pic::Thunkable>> {
        if let Some(relop) = Self::find_rip_relative_operand(&self.disassembler.operand) {
            self.handle_rip_relative_instruction(state, relop)
        } else if let Some(relop) = Self::find_branch_relative_operand(&self.disassembler.operand) {
            self.handle_relative_branch(state, relop)
        } else {
            if Self::is_return(state.mnemonic) {
                // In case the operand is not placed in a branch, the function
                // returns unconditionally, which means that it terminates here.
                self.finished = !self.is_instruction_in_branch(state);
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
    /// mov eax, [rip+0x4892] ; theoretical adjustment after relocation
    /// ```
    unsafe fn handle_rip_relative_instruction(&mut self,
                                              state: &State,
                                              relative_operand: udis::ud_operand)
                                              -> Result<Box<pic::Thunkable>> {
        // If the instruction is an absolute indirect jump, processing stops here
        self.finished = Self::is_jmp(state.mnemonic);

        // The operands displacement (e.g `mov eax, [rip+0x10]` ⟶ 0x10)
        let displacement = relative_operand.lval.sdword as isize;

        // Nothing should be done if `displacement` is within the prolog.
        if (-(self.total_bytes_disassembled as isize)..0).contains(displacement) {
            return Ok(Box::new(state.instruction.to_vec()));
        }

        // These need to be captured by the closure
        let instruction_address = state.instruction.as_ptr() as isize;
        let instruction_bytes = state.instruction.to_vec();

        Ok(Box::new(pic::UnsafeThunk::new(move |offset| {
            let mut bytes = instruction_bytes.clone();

            // Calculate the new relative displacement for the operand. The
            // instruction is relative so the offset (i.e where the trampoline is
            // allocated), must be within a range of +/- 2GB.
            let adjusted_displacement = instruction_address
                .wrapping_sub(offset as isize)
                .wrapping_add(displacement);

            let operand_range = (i32::min_value() as isize)..(i32::max_value() as isize);
            assert!(operand_range.contains(adjusted_displacement));

            // The displacement value is placed at (instruction - disp32)
            let index = instruction_bytes.len() - mem::size_of::<u32>();

            // Write the adjusted displacement offset to the operand
            let as_bytes: [u8; 4] = mem::transmute(adjusted_displacement as u32);
            bytes[index..instruction_bytes.len()].copy_from_slice(&as_bytes);
            bytes
        }, state.instruction.len())))
    }

    // TODO: Add test for unsupported branches
    /// Processes relative branches (e.g `call`, `loop`, `jne`).
    unsafe fn handle_relative_branch(&mut self,
                                     state: &State,
                                     relative_operand: udis::ud_operand)
                                     -> Result<Box<pic::Thunkable>> {
        // Acquire the immediate value from the operand
        let relative_offset = match relative_operand.size {
            8  => relative_operand.lval.ubyte as usize,
            32 => relative_operand.lval.udword as usize,
            _  => unreachable!(),
        };

        // Calculate the absolute address of the target destination
        let destination_address_abs = state.next_instruction_address() as usize + relative_offset;

        if state.mnemonic == udis::ud_mnemonic_code::UD_Icall {
            // Calls are not an issue since they return to the original address
            return Ok(thunk::call(mem::transmute(destination_address_abs)));
        }

        let prolog_range = (self.target as usize)..(self.target as usize + self.margin);

        // If the relative jump is internal, and short enough to
        // fit within the copied function prolog (i.e `margin`),
        // the jump bytes can be copied indiscriminately.
        if prolog_range.contains(destination_address_abs) {
            // Keep track of the jump's destination address
            self.branch_address = Some(destination_address_abs);
            Ok(Box::new(state.instruction.to_vec()))
        } else if Self::is_loop(state.mnemonic) {
            // Loops (e.g 'loopnz', 'jecxz') to the outside are not supported
            Err(ErrorKind::ExternalLoop.into())
        } else if Self::is_jmp(state.mnemonic) {
            // If the function is not in a branch, and it unconditionally jumps
            // a distance larger than the prolog, it's the same as if it terminates.
            self.finished = !self.is_instruction_in_branch(state);
            Ok(thunk::jmp(mem::transmute(destination_address_abs)))
        } else /* Conditional jumps (Jcc) */ {
            // To extract the condition, the primary opcode is required. Short
            // jumps are only one byte, but long jccs are prefixed with 0x0F.
            let primary_opcode = state.instruction.iter().find(|op| **op != 0x0F).unwrap();

            // Extract the condition (i.e 0x74 is [jz rel8] ⟶ 0x74 & 0x0F == 4)
            let condition = primary_opcode & 0x0F;
            Ok(thunk::jcc(mem::transmute(destination_address_abs), condition))
        }
    }

    /// Returns whether the current instruction is inside a branch or not.
    fn is_instruction_in_branch(&self, state: &State) -> bool {
        self.branch_address.map_or(false, |offset| (state.instruction.as_ptr() as usize) < offset)
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

    /// Returns true if the opcode is any type of loop.
    fn is_loop(mnemonic: udis::ud_mnemonic_code) -> bool {
        matches!(mnemonic,
                 udis::ud_mnemonic_code::UD_Iloop   |
                 udis::ud_mnemonic_code::UD_Iloope  |
                 udis::ud_mnemonic_code::UD_Iloopne |
                 udis::ud_mnemonic_code::UD_Ijecxz  |
                 udis::ud_mnemonic_code::UD_Ijcxz)
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

/// State describing the current instruction being processed.
#[derive(Debug)]
struct State {
    instruction: &'static [u8],
    mnemonic: udis::ud_mnemonic_code,
}

impl State {
    /// Returns the address of the next instruction.
    unsafe fn next_instruction_address(&self) -> *const u8 {
        self.instruction.as_ptr().offset(self.instruction.len() as isize)
    }
}
