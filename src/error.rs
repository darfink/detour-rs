//! Error types and utilities.

use std::{fmt, io};

/// The result of a detour operation.
pub type Result<T> = std::result::Result<T, Error>;

/// A representation of all possible errors.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
  /// The address for the target and detour are identical.
  SameAddress,
  /// The address does not contain valid instructions.
  InvalidCode,
  /// The address has no available area for patching.
  NoPatchArea,
  /// The address is not executable memory.
  NotExecutable,
  /// The detour is not initialized.
  NotInitialized,
  /// The detour is already initialized.
  AlreadyInitialized,
  /// No executable memory could be allocated within range of the target.
  OutOfMemory,
  /// The address contains an instruction that cannot be relocated.
  UnsupportedInstruction,
  /// The target's code was modified by a third party since the detour was
  /// created (e.g. another detour, sharing the same target, is still active).
  TargetModified,
  /// A memory operation failed.
  Memory(io::Error),
}

impl std::error::Error for Error {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Error::Memory(error) => Some(error),
      _ => None,
    }
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Error::SameAddress => write!(f, "target and detour address is the same"),
      Error::InvalidCode => write!(f, "address contains invalid assembly"),
      Error::NoPatchArea => write!(f, "cannot find an inline patch area"),
      Error::NotExecutable => write!(f, "address is not executable"),
      Error::NotInitialized => write!(f, "detour is not initialized"),
      Error::AlreadyInitialized => write!(f, "detour is already initialized"),
      Error::OutOfMemory => write!(f, "cannot allocate executable memory near the target"),
      Error::UnsupportedInstruction => write!(f, "address contains an unsupported instruction"),
      Error::TargetModified => write!(f, "target has been modified by a third party"),
      Error::Memory(_) => write!(f, "memory operation failed"),
    }
  }
}

impl From<io::Error> for Error {
  fn from(error: io::Error) -> Self {
    Error::Memory(error)
  }
}
