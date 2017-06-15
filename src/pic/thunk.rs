use generic_array::{GenericArray, ArrayLength};
use pic;
use super::Thunkable;

/// A closure that generates a thunk.
pub struct StaticThunk<N: ArrayLength<u8>>(Box<Fn(usize) -> GenericArray<u8, N>>);

impl<N: ArrayLength<u8>> StaticThunk<N> {
    /// Constructs a new thunk with a specific closure.
    pub fn new<T: Fn(usize) -> GenericArray<u8, N> + 'static>(callback: T) -> Self {
        StaticThunk(Box::new(callback))
    }
}

/// Thunks implement the thunkable interface.
impl<N: ArrayLength<u8>> pic::Thunkable for StaticThunk<N> {
    fn generate(&self, address: usize) -> Vec<u8> {
        self.0(address).to_vec()
    }

    fn len(&self) -> usize {
        N::to_usize()
    }
}

/// A closure that generates a thunk.
pub struct UnsafeThunk {
    callback: Box<Fn(usize) -> Vec<u8>>,
    size: usize
}

/// An unsafe thunk, because it cannot assert at compile time that the generated
/// data is the same size as `len()` (will panic otherwise when emitted).
impl UnsafeThunk {
    /// Constructs a new dynamic thunk with a closure.
    pub unsafe fn new<T: Fn(usize) -> Vec<u8> + 'static>(callback: T, size: usize) -> Self {
        UnsafeThunk {
            callback: Box::new(callback),
            size: size,
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
