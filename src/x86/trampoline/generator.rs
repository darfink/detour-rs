use std::mem;
use error::*;

use x86::thunk;
use pic;
use super::disasm::*;

/// Processes a function until `margin` bytes have been disassembled.
pub unsafe fn generate(target: *const (), margin: usize) -> Result<(pic::CodeBuilder, usize)> {
    Generator {
        disassembler: Disassembler::new(target),
        total_bytes_disassembled: 0,
        branch_address: None,
        finished: false,
        target: target,
        margin: margin,
    }.process()
}

/// A trampoline generator (x86/x64).
struct Generator {
    disassembler: Disassembler,
    total_bytes_disassembled: usize,
    branch_address: Option<usize>,
    finished: bool,
    target: *const (),
    margin: usize,
}

// TODO: should margins larger than 5 bytes be accounted for?
impl Generator {
    /// Internal implementation for the `process` function.
    unsafe fn process(mut self) -> Result<(pic::CodeBuilder, usize)> {
        let mut builder = pic::CodeBuilder::new();

        while !self.finished {
            let instruction = self.next_instruction()?;
            let thunk = self.process_instruction(&instruction)?;

            // If the trampoline displacement is larger than the target function,
            // all instructions will be offset, and if there is internal branching,
            // it will end up at the wrong instructions.
            if self.is_instruction_in_branch(&instruction) && instruction.len() != thunk.len() {
                bail!(ErrorKind::UnsupportedRelativeBranch);
            } else {
                builder.add_thunk(thunk);
            }

            // Determine whether enough bytes for the margin has been disassembled
            if self.total_bytes_disassembled >= self.margin && !self.finished {
                // Add a jump to the first instruction after the prolog
                builder.add_thunk(thunk::jmp(instruction.next_instruction_address()));
                self.finished = true;
            }
        }

        Ok((builder, self.total_bytes_disassembled))
    }

    /// Disassembles the next instruction and returns the new state.
    unsafe fn next_instruction(&mut self) -> Result<Instruction> {
        let instruction_address = self.target as usize + self.total_bytes_disassembled;

        // Disassemble the next instruction
        match Instruction::new(&mut self.disassembler, instruction_address as *const _) {
            None => bail!(ErrorKind::InvalidCode),
            Some(instruction) => {
                // Keep track of the total amount of bytes
                self.total_bytes_disassembled += instruction.len();
                Ok(instruction)
            },
        }
    }

    /// Analyses and modifies an instruction if required.
    unsafe fn process_instruction(&mut self, instruction: &Instruction) -> Result<Box<pic::Thunkable>> {
        if let Some(displacement) = instruction.rip_operand_displacement() {
            self.handle_rip_relative_instruction(instruction, displacement)
        } else if let Some(offset) = instruction.relative_branch_offset() {
            self.handle_relative_branch(instruction, offset)
        } else {
            if instruction.is_return() {
                // In case the operand is not placed in a branch, the function
                // returns unconditionally, (i.e it terminates here).
                self.finished = !self.is_instruction_in_branch(instruction);
            }

            // The instruction does not use any position-dependant operands,
            // therefore the bytes can be copied directly from source.
            Ok(Box::new(instruction.as_slice().to_vec()))
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
                                              instruction: &Instruction,
                                              displacement: isize)
                                              -> Result<Box<pic::Thunkable>> {
        // If the instruction is an indirect jump, processing stops here
        self.finished = instruction.is_unconditional_jump();

        // Nothing should be done if `displacement` is within the prolog.
        if (-(self.total_bytes_disassembled as isize)..0).contains(displacement) {
            return Ok(Box::new(instruction.as_slice().to_vec()));
        }

        // These need to be captured by the closure
        let instruction_address = instruction.address() as isize;
        let instruction_bytes = instruction.as_slice().to_vec();

        Ok(Box::new(pic::UnsafeThunk::new(move |offset| {
            let mut bytes = instruction_bytes.clone();

            // Calculate the new relative displacement for the operand. The
            // instruction is relative so the offset (i.e where the trampoline is
            // allocated), must be within a range of +/- 2GB.
            let adjusted_displacement = instruction_address
                .wrapping_sub(offset as isize)
                .wrapping_add(displacement);
            assert!(::x86::is_within_2gb(adjusted_displacement));

            // The displacement value is placed at (instruction - disp32)
            let index = instruction_bytes.len() - mem::size_of::<u32>();

            // Write the adjusted displacement offset to the operand
            let as_bytes: [u8; 4] = mem::transmute(adjusted_displacement as u32);
            bytes[index..instruction_bytes.len()].copy_from_slice(&as_bytes);
            bytes
        }, instruction.len())))
    }

    /// Processes relative branches (e.g `call`, `loop`, `jne`).
    unsafe fn handle_relative_branch(&mut self,
                                     instruction: &Instruction,
                                     offset: isize)
                                     -> Result<Box<pic::Thunkable>> {
        // Calculate the absolute address of the target destination
        let destination_address_abs = instruction.next_instruction_address() + offset as usize;

        if instruction.is_call() {
            // Calls are not an issue since they return to the original address
            return Ok(thunk::call(destination_address_abs));
        }

        let prolog_range = (self.target as usize)..(self.target as usize + self.margin);

        // If the relative jump is internal, and short enough to
        // fit within the copied function prolog (i.e `margin`),
        // the jump bytes can be copied indiscriminately.
        if prolog_range.contains(destination_address_abs) {
            // Keep track of the jump's destination address
            self.branch_address = Some(destination_address_abs);
            Ok(Box::new(instruction.as_slice().to_vec()))
        } else if instruction.is_loop() {
            // Loops (e.g 'loopnz', 'jecxz') to the outside are not supported
            Err(ErrorKind::ExternalLoop.into())
        } else if instruction.is_unconditional_jump() {
            // If the function is not in a branch, and it unconditionally jumps
            // a distance larger than the prolog, it's the same as if it terminates.
            self.finished = !self.is_instruction_in_branch(instruction);
            Ok(thunk::jmp(destination_address_abs))
        } else /* Conditional jumps (Jcc) */ {
            // To extract the condition, the primary opcode is required. Short
            // jumps are only one byte, but long jccs are prefixed with 0x0F.
            let primary_opcode = instruction.as_slice().iter().find(|op| **op != 0x0F).unwrap();

            // Extract the condition (i.e 0x74 is [jz rel8] ⟶ 0x74 & 0x0F == 4)
            let condition = primary_opcode & 0x0F;
            Ok(thunk::jcc(destination_address_abs, condition))
        }
    }

    /// Returns whether the current instruction is inside a branch or not.
    fn is_instruction_in_branch(&self, instruction: &Instruction) -> bool {
        self.branch_address.map_or(false, |offset| instruction.address() < offset)
    }
}
