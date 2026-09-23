//! Platforms without thread suspension support (yet).

use super::Thread;
use crate::error::{Error, Result};
use alloc::vec::Vec;

pub(crate) type RawThread = usize;

#[cfg(all(unix, feature = "std"))]
pub(super) fn from_pthread(thread: libc::pthread_t) -> Thread {
  Thread(thread as usize)
}

/// A suspended thread (never constructed).
pub(crate) enum Suspended {}

impl Suspended {
  pub fn pc(&self) -> usize {
    match *self {}
  }

  pub fn set_pc(&mut self, _pc: usize) {
    match *self {}
  }

  pub(super) fn apply(&mut self) -> Result<()> {
    match *self {}
  }
}

pub(super) fn suspend_all() -> Result<Vec<Suspended>> {
  Err(Error::ThreadsUnsupported)
}

pub(super) fn suspend(threads: &[Thread]) -> Result<Vec<Suspended>> {
  if threads.is_empty() {
    Ok(Vec::new())
  } else {
    Err(Error::ThreadsUnsupported)
  }
}
