//! Error types and utilities.

use core::fmt;

/// The result of a detour operation.
pub type Result<T> = core::result::Result<T, Error>;

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
  Memory(MemoryError),
}

impl core::error::Error for Error {
  fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
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

impl From<MemoryError> for Error {
  fn from(error: MemoryError) -> Self {
    Error::Memory(error)
  }
}

/// A failed memory operation (e.g. querying or protecting memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryError(Kind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
  /// An operating system error code (`errno`, or `GetLastError`).
  Os(i32),
  /// A Mach kernel error code (`kern_return_t`).
  Mach(i32),
  /// Any other failure.
  Other(&'static str),
}

impl MemoryError {
  #[cfg(target_vendor = "apple")]
  pub(crate) const fn mach(code: i32) -> Self {
    MemoryError(Kind::Mach(code))
  }

  /// Returns the operating system error code (`errno` on Unix-like platforms,
  /// or `GetLastError` on Windows), if applicable.
  pub fn raw_os_error(&self) -> Option<i32> {
    match self.0 {
      Kind::Os(code) => Some(code),
      _ => None,
    }
  }
}

impl fmt::Display for MemoryError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match &self.0 {
      #[cfg(feature = "std")]
      Kind::Os(code) => write!(f, "{}", std::io::Error::from_raw_os_error(*code)),
      #[cfg(not(feature = "std"))]
      Kind::Os(code) => write!(f, "system call failed ({code})"),
      Kind::Mach(code) => write!(f, "mach kernel call failed ({code})"),
      Kind::Other(message) => f.write_str(message),
    }
  }
}

impl core::error::Error for MemoryError {}

impl From<region::Error> for MemoryError {
  fn from(error: region::Error) -> Self {
    MemoryError(match error {
      region::Error::SystemCall(code) => Kind::Os(code),
      region::Error::MachCall(code) => Kind::Mach(code),
      region::Error::UnmappedRegion => Kind::Other("memory is unmapped"),
      region::Error::InvalidParameter(_) => Kind::Other("invalid memory parameter"),
      region::Error::ProcfsInput(_) => Kind::Other("invalid procfs input"),
    })
  }
}

impl From<region::Error> for Error {
  fn from(error: region::Error) -> Self {
    Error::Memory(error.into())
  }
}

#[cfg(feature = "std")]
impl From<MemoryError> for std::io::Error {
  fn from(error: MemoryError) -> Self {
    match error.0 {
      Kind::Os(code) => std::io::Error::from_raw_os_error(code),
      _ => std::io::Error::other(error),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use alloc::string::ToString;

  #[test]
  fn memory_error_preserves_os_code() {
    let error = MemoryError::from(region::Error::SystemCall(13));
    assert_eq!(error.raw_os_error(), Some(13));
    assert!(!error.to_string().is_empty());

    #[cfg(feature = "std")]
    assert_eq!(std::io::Error::from(error).raw_os_error(), Some(13));
  }

  #[test]
  fn error_exposes_source() {
    let error = Error::from(region::Error::MachCall(2));
    let source = core::error::Error::source(&error).expect("memory errors have a source");
    assert_eq!(source.to_string(), "mach kernel call failed (2)");
    assert!(core::error::Error::source(&Error::OutOfMemory).is_none());
  }
}
