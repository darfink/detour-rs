//! Error types and utilities.

use std::error::Error as StdError;
use std::fmt;

/// The result of a detour operation.
pub type Result<T> = ::std::result::Result<T, Error>;

/// A representation of all possible errors.
#[derive(Debug)]
pub enum Error {
  /// The address for the target and detour are identical
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
  /// The system is out of executable memory.
  OutOfMemory,
  /// The address contains an instruction that prevents detouring.
  UnsupportedInstruction,
  /// A memory operation failed.
  RegionFailure(region::Error),
}

impl StdError for Error {
  fn source(&self) -> Option<&(dyn StdError + 'static)> {
    if let Error::RegionFailure(error) = self {
      Some(error)
    } else {
      None
    }
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Error::SameAddress => write!(f, "Target and detour address is the same"),
      Error::InvalidCode => write!(f, "Address contains invalid assembly"),
      Error::NoPatchArea => write!(f, "Cannot find an inline patch area"),
      Error::NotExecutable => write!(f, "Address is not executable"),
      Error::NotInitialized => write!(f, "Detour is not initialized"),
      Error::AlreadyInitialized => write!(f, "Detour is already initialized"),
      Error::OutOfMemory => write!(f, "Cannot allocate memory"),
      Error::UnsupportedInstruction => write!(f, "Address contains an unsupported instruction"),
      Error::RegionFailure(ref error) => write!(f, "{}", error),
    }
  }
}

impl From<region::Error> for Error {
  fn from(error: region::Error) -> Self {
    Error::RegionFailure(error)
  }
}
