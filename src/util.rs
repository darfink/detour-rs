use std::ops::Range;
use crate::error::Result;

/// Returns true if an address is executable.
pub fn is_executable_address(address: *const ()) -> Result<bool> {
  Ok(
    region::query(address as *const _)?
      .protection
      .contains(region::Protection::Execute),
  )
}

/// Trait for ranges containing values.
pub trait RangeContains<Idx: PartialOrd<Idx>> {
  fn contains_(&self, item: Idx) -> bool;
}

impl<Idx: PartialOrd<Idx>> RangeContains<Idx> for Range<Idx> {
  fn contains_(&self, item: Idx) -> bool {
    self.start <= item && self.end > item
  }
}
