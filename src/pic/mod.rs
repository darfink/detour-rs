pub use self::emitter::CodeEmitter;
pub use self::thunk::{FixedThunk, UnsafeThunk};

mod emitter;
mod thunk;

/// An interface for generating PIC thunks.
pub trait Thunkable {
  /// Generates the code at the specified address.
  fn generate(&self, address: usize) -> Vec<u8>;

  /// Returns the size of a generated thunk.
  fn len(&self) -> usize;
}

/// Thunkable implementation for static data
impl Thunkable for Vec<u8> {
  /// Generates a static thunk assumed to be PIC
  fn generate(&self, _address: usize) -> Vec<u8> {
    self.clone()
  }

  /// Returns the size of a generated thunk
  fn len(&self) -> usize {
    self.len()
  }
}
