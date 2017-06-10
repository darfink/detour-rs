use error::*;
use pic;

mod disasm;
mod generator;

/// An interface for creating a trampoline to a function.
pub struct Trampoline {
    builder: pic::CodeBuilder,
    prolog_size: usize,
}

impl Trampoline {
    /// Constructs a new trampoline for the specified function.
    pub unsafe fn new(target: *const (), margin: usize) -> Result<Trampoline> {
        let (builder, prolog_size) = generator::generate(target, margin)?;
        Ok(Trampoline {
            prolog_size: prolog_size,
            builder: builder,
        })
    }

    /// Returns a reference to the trampoline's code generator.
    pub fn builder(&self) -> &pic::CodeBuilder {
        &self.builder
    }

    /// Returns the size of the prolog (i.e the amount of disassembled bytes).
    pub fn prolog_size(&self) -> usize {
        self.prolog_size
    }
}
