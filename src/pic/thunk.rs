use super::Thunkable;
use generic_array::{ArrayLength, GenericArray};

/// A closure that generates a thunk.
pub struct FixedThunk<N: ArrayLength<u8>>(Box<dyn Fn(usize) -> GenericArray<u8, N>>);

impl<N: ArrayLength<u8>> FixedThunk<N> {
  /// Constructs a new thunk with a specific closure.
  pub fn new<T: Fn(usize) -> GenericArray<u8, N> + 'static>(callback: T) -> Self {
    FixedThunk(Box::new(callback))
  }
}

/// Thunks implement the thunkable interface.
impl<N: ArrayLength<u8>> Thunkable for FixedThunk<N> {
  fn generate(&self, address: usize) -> Vec<u8> {
    self.0(address).to_vec()
  }

  fn len(&self) -> usize {
    N::to_usize()
  }
}

/// A closure that generates an unsafe thunk.
pub struct UnsafeThunk {
  callback: Box<dyn Fn(usize) -> Vec<u8>>,
  size: usize,
}

/// An unsafe thunk, because it cannot be asserted at compile time, that the
/// generated data is the same size as `len()` (will panic otherwise when
/// emitted).
impl UnsafeThunk {
  /// Constructs a new dynamic thunk with a closure.
  pub unsafe fn new<T: Fn(usize) -> Vec<u8> + 'static>(callback: T, size: usize) -> Self {
    UnsafeThunk {
      callback: Box::new(callback),
      size,
    }
  }
}

impl Thunkable for UnsafeThunk {
  /// Generates a dynamic thunk, assumed to be PIC.
  fn generate(&self, address: usize) -> Vec<u8> {
    (self.callback)(address)
  }

  /// Returns the size of the generated thunk.
  fn len(&self) -> usize {
    self.size
  }
}
