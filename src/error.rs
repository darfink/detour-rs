//! Error types and utilities.

use {failure, region};

/// The result of a detour operation.
pub type Result<T> = ::std::result::Result<T, failure::Error>;

#[derive(Fail, Debug)]
pub enum Error {
    /// A static detour has already been initialized
    #[fail(display = "detour has already been initialized")]
    AlreadyExisting,
    /// The address for the target and detour are identical
    #[fail(display = "target and detour address is the same")]
    SameAddress,
    /// The address does not contain valid instructions.
    #[fail(display = "address contains invalid assembly")]
    InvalidCode,
    /// The address has no available area for patching.
    #[fail(display = "cannot find an inline patch area")]
    NoPatchArea,
    /// The address is not executable memory.
    #[fail(display = "address is not executable")]
    NotExecutable,
    /// The system is out of executable memory.
    #[fail(display = "cannot allocate memory")]
    OutOfMemory,
    /// The address contains an instruction that prevents detouring.
    #[fail(display = "address contains an unsupported instruction")]
    UnsupportedInstruction,
    // A memory operation failed.
    #[fail(display = "{}", _0)]
    RegionFailure(#[cause] region::Error)
}
