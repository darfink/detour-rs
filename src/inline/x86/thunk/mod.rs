#![allow(dead_code)]
use generic_array::{GenericArray, ArrayLength};
use inline::pic;

/// Implements x86 operations
pub mod x86;

/// Implements x64 operations
#[cfg(target_arch = "x86_64")]
pub mod x64;

#[cfg(target_arch = "x86")]
mod arch {
    pub use super::x86::jmp_rel32 as jmp;
    pub use super::x86::call_rel32 as call;
    pub use super::x86::jcc_rel32 as jcc;
}

#[cfg(target_arch = "x86_64")]
mod arch {
    pub use super::x64::jmp_abs as jmp;
    pub use super::x64::call_abs as call;
    pub use super::x64::jcc_abs as jcc;
}

// Export the default architecture
pub use self::arch::*;

/// A closure that generates a thunk.
pub struct Thunk<N: ArrayLength<u8>>(Box<Fn(usize) -> GenericArray<u8, N>>);

impl<N: ArrayLength<u8>> Thunk<N> {
    /// Constructs a new thunk with a specific closure.
    fn new<T: Fn(usize) -> GenericArray<u8, N> + 'static>(callback: T) -> Self {
        Thunk(Box::new(callback))
    }
}

/// Thunks implement the thunkable interface.
impl<N: ArrayLength<u8>> pic::Thunkable for Thunk<N> {
    fn generate(&self, address: usize) -> Vec<u8> {
        self.0(address).to_vec()
    }

    fn len(&self) -> usize {
        N::to_usize()
    }
}
