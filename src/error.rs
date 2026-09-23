//! Error types and utilities.

use core::fmt;

/// The result of a detour operation.
pub type Result<T> = core::result::Result<T, Error>;

/// A representation of all possible errors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
  /// The address for the target and detour are identical.
  SameAddress,
  /// The target does not contain valid instructions.
  InvalidInstruction,
  /// The target is too small to fit a branch (and cannot be hot patched).
  PatchAreaTooSmall,
  /// The address is not executable memory.
  NotExecutable,
  /// The detour is not initialized.
  NotInitialized,
  /// The detour is already initialized.
  AlreadyInitialized,
  /// No executable memory could be allocated within branch range of the
  /// target.
  NoNearbyMemory,
  /// The target contains an instruction that cannot be relocated.
  UnsupportedInstruction,
  /// The target's code was modified by a third party since the detour was
  /// created (e.g. another detour, sharing the same target, is still active).
  TargetModified,
  /// A suspended thread is executing code that cannot be relocated, i.e. an
  /// instruction that was rewritten when it was copied to the trampoline.
  ThreadNotRelocatable,
  /// Threads cannot be suspended on this platform.
  ThreadsUnsupported,
  /// A memory operation failed.
  Memory(OsError),
  /// Suspending, inspecting, or resuming a thread failed.
  Thread(OsError),
}

impl core::error::Error for Error {
  fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
    match self {
      Error::Memory(error) | Error::Thread(error) => Some(error),
      _ => None,
    }
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Error::SameAddress => f.write_str("target and detour addresses are the same"),
      Error::InvalidInstruction => f.write_str("target contains an invalid instruction"),
      Error::PatchAreaTooSmall => f.write_str("target is too small to be patched"),
      Error::NotExecutable => f.write_str("address is not executable"),
      Error::NotInitialized => f.write_str("detour is not initialized"),
      Error::AlreadyInitialized => f.write_str("detour is already initialized"),
      Error::NoNearbyMemory => f.write_str("cannot allocate executable memory near the target"),
      Error::UnsupportedInstruction => f.write_str("target contains an unsupported instruction"),
      Error::TargetModified => f.write_str("target has been modified by a third party"),
      Error::ThreadNotRelocatable => {
        f.write_str("a suspended thread is executing code that cannot be relocated")
      },
      Error::ThreadsUnsupported => f.write_str("threads cannot be suspended on this platform"),
      Error::Memory(_) => f.write_str("memory operation failed"),
      Error::Thread(_) => f.write_str("thread operation failed"),
    }
  }
}

impl Error {
  /// Converts a failed memory operation of `region`.
  pub(crate) fn from_region(error: region::Error) -> Self {
    Error::Memory(OsError::from_region(error))
  }
}

/// A failed operating system call (e.g. protecting memory, or suspending a
/// thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsError(Kind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
  /// An operating system error code (`errno`, or `GetLastError`).
  Os(i32),
  /// A Mach kernel error code (`kern_return_t`).
  Mach(i32),
  /// Any other failure.
  Other(&'static str),
}

impl OsError {
  #[cfg(target_vendor = "apple")]
  pub(crate) const fn mach(code: i32) -> Self {
    OsError(Kind::Mach(code))
  }

  /// Returns the last error of the current thread (`GetLastError`).
  #[cfg(windows)]
  pub(crate) fn last_os_error() -> Self {
    // SAFETY: Reads the calling thread's last-error code.
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    OsError(Kind::Os(code as i32))
  }

  /// Returns the operating system error code (`errno` on Unix-like platforms,
  /// or `GetLastError` on Windows), if applicable.
  pub fn raw_os_error(&self) -> Option<i32> {
    match self.0 {
      Kind::Os(code) => Some(code),
      _ => None,
    }
  }

  /// Returns the Mach kernel error code (`kern_return_t`), if applicable.
  ///
  /// This is only ever set on Apple platforms.
  pub fn raw_mach_error(&self) -> Option<i32> {
    match self.0 {
      Kind::Mach(code) => Some(code),
      _ => None,
    }
  }

  /// Converts an error from `region`.
  ///
  /// Intentionally not a `From` implementation, so that `region` remains a
  /// private dependency.
  pub(crate) fn from_region(error: region::Error) -> Self {
    OsError(match error {
      region::Error::SystemCall(code) => Kind::Os(code),
      region::Error::MachCall(code) => Kind::Mach(code),
      region::Error::UnmappedRegion => Kind::Other("memory is unmapped"),
      region::Error::InvalidParameter(_) => Kind::Other("invalid memory parameter"),
      region::Error::ProcfsInput(_) => Kind::Other("invalid procfs input"),
    })
  }
}

impl fmt::Display for OsError {
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

impl core::error::Error for OsError {}

#[cfg(feature = "std")]
impl From<OsError> for std::io::Error {
  fn from(error: OsError) -> Self {
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
    let error = OsError::from_region(region::Error::SystemCall(13));
    assert_eq!(error.raw_os_error(), Some(13));
    assert_eq!(error.raw_mach_error(), None);
    assert!(!error.to_string().is_empty());

    #[cfg(feature = "std")]
    assert_eq!(std::io::Error::from(error).raw_os_error(), Some(13));
  }

  #[test]
  fn error_exposes_source() {
    let error = Error::Memory(OsError::from_region(region::Error::MachCall(2)));
    assert_eq!(error.clone(), error);
    let source = core::error::Error::source(&error).expect("memory errors have a source");
    assert_eq!(source.to_string(), "mach kernel call failed (2)");
    assert!(core::error::Error::source(&Error::NoNearbyMemory).is_none());
  }
}
