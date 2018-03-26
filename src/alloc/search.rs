use error::*;
use region;
use std::ops::Range;
use util::RangeContains;

/// Returns an iterator for free before the specified address.
pub fn before(origin: *const (), range: Option<Range<usize>>) -> RegionIter {
  RegionIter::new(origin, range, SearchDirection::Before)
}

/// Returns an iterator for free after the specified address.
pub fn after(origin: *const (), range: Option<Range<usize>>) -> RegionIter {
  RegionIter::new(origin, range, SearchDirection::After)
}

/// Direction for the region search.
enum SearchDirection {
  Before,
  After,
}

/// An iterator searching for free regions.
pub struct RegionIter {
  range: Range<usize>,
  search: SearchDirection,
  current: usize,
}

impl RegionIter {
  /// Creates a new iterator for free regions.
  fn new(origin: *const (), range: Option<Range<usize>>, search: SearchDirection) -> Self {
    RegionIter {
      range: range.unwrap_or(0..usize::max_value()),
      current: origin as usize,
      search,
    }
  }
}

impl Iterator for RegionIter {
  type Item = Result<*const ()>;

  /// Returns the closest free region for the current address.
  fn next(&mut self) -> Option<Self::Item> {
    let page_size = region::page::page_size();

    while self.current > 0 && self.range.contains_(self.current) {
      match region::query(self.current as *const _) {
        Ok(region) => {
          self.current = match self.search {
            SearchDirection::Before => region.lower().saturating_sub(page_size),
            SearchDirection::After => region.upper(),
          }
        },
        Err(error) => {
          // Check whether the region is free, otherwise return the error
          let result = Some(match error.downcast().expect("downcasting region error") {
            region::Error::Free => Ok(self.current as *const _),
            inner @ _ => Err(Error::RegionFailure(inner).into()),
          });

          // Adjust the offset for repeated calls.
          match self.search {
            SearchDirection::Before => {
              self.current.saturating_sub(page_size);
            },
            SearchDirection::After => self.current += page_size,
          }

          return result;
        },
      }
    }

    None
  }
}

#[cfg(test)]
mod tests {}
