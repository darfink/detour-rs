use crate::error::{Error, Result};
use std::ops::Range;

/// Returns an iterator for free after the specified address.
pub fn after(
  origin: *const (),
  range: Option<Range<usize>>,
) -> impl Iterator<Item = Result<*const ()>> {
  FreeRegionIter::new(origin, range, SearchDirection::After)
}

/// Returns an iterator for free before the specified address.
pub fn before(
  origin: *const (),
  range: Option<Range<usize>>,
) -> impl Iterator<Item = Result<*const ()>> {
  FreeRegionIter::new(origin, range, SearchDirection::Before)
}

/// Direction for the region search.
enum SearchDirection {
  Before,
  After,
}

/// An iterator searching for free regions.
struct FreeRegionIter {
  range: Range<usize>,
  search: SearchDirection,
  current: usize,
}

impl FreeRegionIter {
  /// Creates a new iterator for free regions.
  fn new(origin: *const (), range: Option<Range<usize>>, search: SearchDirection) -> Self {
    FreeRegionIter {
      range: range.unwrap_or(0..usize::max_value()),
      current: origin as usize,
      search,
    }
  }
}

impl Iterator for FreeRegionIter {
  type Item = Result<*const ()>;

  /// Returns the closest free region for the current address.
  fn next(&mut self) -> Option<Self::Item> {
    let page_size = region::page::size();

    while self.current > 0 && self.range.contains(&self.current) {
      match region::query(self.current as *const _) {
        Ok(region) => {
          self.current = match self.search {
            SearchDirection::Before => region.lower().saturating_sub(page_size),
            SearchDirection::After => region.upper(),
          }
        },
        Err(error) => {
          // Check whether the region is free, otherwise return the error
          let result = Some(match error {
            region::Error::FreeMemory => Ok(self.current as *const _),
            inner => Err(Error::RegionFailure(inner)),
          });

          // Adjust the offset for repeated calls.
          self.current = match self.search {
            SearchDirection::Before => self.current.saturating_sub(page_size),
            SearchDirection::After => self.current + page_size,
          };

          return result;
        },
      }
    }

    None
  }
}
