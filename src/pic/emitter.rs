use super::Thunkable;

/// An interface for generating PIC.
pub struct CodeEmitter {
  thunks: Vec<Box<dyn Thunkable>>,
}

/// Used for combining PIC segments.
impl CodeEmitter {
  /// Constructs a new code emitter.
  pub fn new() -> Self {
    CodeEmitter { thunks: Vec::new() }
  }

  /// Generates code for use at the specified address.
  pub fn emit(&self, base: *const ()) -> Vec<u8> {
    let mut result = Vec::with_capacity(self.len());
    let mut base = base as usize;

    for thunk in &self.thunks {
      // Retrieve the code for the segment
      let code = thunk.generate(base);
      assert_eq!(code.len(), thunk.len());

      // Advance the current EIP address
      base += thunk.len();
      result.extend(code);
    }

    result
  }

  /// Adds a position-independant code segment.
  pub fn add_thunk(&mut self, thunk: Box<dyn Thunkable>) {
    self.thunks.push(thunk);
  }

  /// Returns the total size of a all code segments.
  pub fn len(&self) -> usize {
    self.thunks.iter().fold(0, |sum, thunk| sum + thunk.len())
  }
}
