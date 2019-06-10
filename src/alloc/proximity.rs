use std::ops::Range;
use std::slice;

use slice_pool::sync::{SliceBox, SlicePool};

use super::search as region_search;
use crate::error::{Error, Result};

/// Defines the allocation type.
pub type Allocation = SliceBox<u8>;

/// Shared instance containing all pools
pub struct ProximityAllocator {
  pub max_distance: usize,
  pub pools: Vec<SlicePool<u8>>,
}

impl ProximityAllocator {
  /// Allocates a slice in an eligible memory map.
  pub fn allocate(&mut self, origin: *const (), size: usize) -> Result<Allocation> {
    let memory_range = ((origin as usize).saturating_sub(self.max_distance))
      ..((origin as usize).saturating_add(self.max_distance));

    // Check if an existing pool can handle the allocation request
    self.allocate_memory(&memory_range, size).or_else(|_| {
      // ... otherwise allocate a pool within the memory range
      self.allocate_pool(&memory_range, origin, size).map(|pool| {
        // Use the newly allocated pool for the request
        let allocation = pool.alloc(size).unwrap();
        self.pools.push(pool);
        allocation
      })
    })
  }

  /// Releases the memory pool associated with an allocation.
  pub fn release(&mut self, value: &Allocation) {
    // Find the associated memory pool
    let index = self
      .pools
      .iter()
      .position(|pool| {
        let lower = pool.as_ptr() as usize;
        let upper = lower + pool.len();

        // Determine if this is the associated memory pool
        (lower..upper).contains(&(value.as_ptr() as usize))
      })
      .expect("retrieving associated memory pool");

    // Release the pool if the associated allocation is unique
    if self.pools[index].len() == 1 {
      self.pools.remove(index);
    }
  }

  /// Allocates a chunk using any of the existing pools.
  fn allocate_memory(&mut self, range: &Range<usize>, size: usize) -> Result<Allocation> {
    // Returns true if the pool's memory is within the range
    let is_pool_in_range = |pool: &SlicePool<u8>| {
      let lower = pool.as_ptr() as usize;
      let upper = lower + pool.len();
      range.contains(&lower) && range.contains(&(upper - 1))
    };

    // Tries to allocate a slice within any eligible pool
    self
      .pools
      .iter_mut()
      .filter_map(|pool| {
        if is_pool_in_range(pool) {
          pool.alloc(size)
        } else {
          None
        }
      })
      .next()
      .ok_or(Error::OutOfMemory)
  }

  /// Allocates a new pool close to `origin`.
  fn allocate_pool(
    &mut self,
    range: &Range<usize>,
    origin: *const (),
    size: usize,
  ) -> Result<SlicePool<u8>> {
    let before = region_search::before(origin, Some(range.clone()));
    let after = region_search::after(origin, Some(range.clone()));

    // TODO: Part of the pool can be out of range
    // Try to allocate after the specified address first (mostly because
    // macOS cannot allocate memory before the process's address).
    after
      .chain(before)
      .filter_map(|result| match result {
        Ok(address) => Self::allocate_fixed_pool(address, size).map(Ok),
        Err(error) => Some(Err(error)),
      })
      .next()
      .unwrap_or(Err(Error::OutOfMemory))
  }

  /// Tries to allocate fixed memory at the specified address.
  fn allocate_fixed_pool(address: *const (), size: usize) -> Option<SlicePool<u8>> {
    // Try to allocate memory at the specified address
    mmap::MemoryMap::new(
      size,
      &[
        mmap::MapOption::MapReadable,
        mmap::MapOption::MapWritable,
        mmap::MapOption::MapExecutable,
        mmap::MapOption::MapAddr(address as *const _),
      ],
    )
    .ok()
    .map(SliceableMemoryMap)
    .map(SlicePool::new)
  }
}

// TODO: Use memmap-rs instead
/// A wrapper for making a memory map compatible with `SlicePool`.
struct SliceableMemoryMap(mmap::MemoryMap);

impl SliceableMemoryMap {
  pub fn as_slice(&self) -> &[u8] {
    unsafe { slice::from_raw_parts(self.0.data(), self.0.len()) }
  }

  pub fn as_mut_slice(&mut self) -> &mut [u8] {
    unsafe { slice::from_raw_parts_mut(self.0.data(), self.0.len()) }
  }
}

impl AsRef<[u8]> for SliceableMemoryMap {
  fn as_ref(&self) -> &[u8] {
    self.as_slice()
  }
}

impl AsMut<[u8]> for SliceableMemoryMap {
  fn as_mut(&mut self) -> &mut [u8] {
    self.as_mut_slice()
  }
}

unsafe impl Send for SliceableMemoryMap {}
unsafe impl Sync for SliceableMemoryMap {}
