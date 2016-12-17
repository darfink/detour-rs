use std::ops::Range;
use std::slice;

use {region, mmap};
use boolinator::Boolinator;
use slice_pool::{SlicePool, PoolVal};
use error::*;

lazy_static! {
    static ref PAGE_SIZE: usize = region::page_size();
}

/// Defines the allocation type.
pub type Allocation = PoolVal<u8>;

/// Shared instance containing all pools
pub struct Allocator {
    pub max_distance: usize,
    pub pools: Vec<SlicePool<u8>>,
}

impl Allocator {
    /// Allocates a slice in an eligible memory map.
    pub fn allocate(&mut self, origin: *const (), size: usize) -> Result<Allocation> {
        let memory_range = ((origin as usize).saturating_sub(self.max_distance))
                         ..((origin as usize).saturating_add(self.max_distance));

        // Check if an existing pool can handle the allocation request
        self.allocate_existing(&memory_range, size).map(Ok).unwrap_or_else(|| {
            // ... otherwise allocate a pool within the memory range
            self.allocate_pool(&memory_range, origin, size).map(|mut pool| {
                // Use the newly allocated pool for the request
                let allocation = pool.allocate(size).unwrap();
                self.pools.push(pool);
                allocation
            })
        })
    }

    /// Releases the memory pool associated with an allocation.
    pub fn release(&mut self, value: &Allocation) {
        // Find the associated memory pool
        let index = self.pools.iter().position(|pool| {
            let lower = pool.as_ptr() as usize;
            let upper = lower + pool.len();

            // Determine if this is the associated memory pool
            (lower..upper).contains(value.as_ptr() as usize)
        }).unwrap();

        // Release the pool if the associated allocation is unique
        if self.pools[index].allocations() == 1 {
            self.pools.remove(index);
        }
    }

    /// Allocates a chunk using any of the existing pools.
    fn allocate_existing(&mut self, range: &Range<usize>, size: usize) -> Option<Allocation> {
        // Returns true if the pool's memory is within the range
        let is_pool_in_range = |pool: &SlicePool<u8>| {
            let lower = pool.as_ptr();
            let upper = unsafe { lower.offset(pool.len() as isize) };
            range.contains(lower as usize) && range.contains(upper as usize - 1)
        };

        // Tries to allocate a slice within any eligible pool
        self.pools.iter_mut()
            .filter_map(|pool| is_pool_in_range(pool).and_option_from(|| pool.allocate(size)))
            .next()
    }

    /// Allocates a new pool close to the `origin`.
    fn allocate_pool(&mut self,
                     range: &Range<usize>,
                     origin: *const (),
                     size: usize) -> Result<SlicePool<u8>> {
        let after = RegionFreeIter::new(origin, Some(range.clone()), RegionSearch::After);
        let before = RegionFreeIter::new(origin, Some(range.clone()), RegionSearch::Before);

        // Try to allocate after the specified address first (mostly because
        // macOS cannot allocate memory before the process's address).
        after.chain(before).filter_map(|result| {
            match result {
                Ok(address) => Self::allocate_region_pool(address, size).map(Ok),
                Err(error) => Some(Err(error)),
            }
        }).next().unwrap_or(Err(ErrorKind::OutOfMemory.into()))
    }

    /// Tries to allocate fixed memory at the specified address.
    fn allocate_region_pool(address: *const (), size: usize) -> Option<SlicePool<u8>> {
        // Try to allocate memory at the specified address
        mmap::MemoryMap::new(Self::page_ceil(size), &[
            mmap::MapOption::MapReadable,
            mmap::MapOption::MapWritable,
            mmap::MapOption::MapExecutable,
            mmap::MapOption::MapAddr(address as *const _),
        ]).ok().map(SliceableMemoryMap).map(SlicePool::new)
    }

    /// Rounds an address up to the closest page boundary.
    fn page_ceil(address: usize) -> usize {
        (address + *PAGE_SIZE - 1) & !(*PAGE_SIZE - 1)
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
    fn as_ref(&self) -> &[u8] { self.as_slice() }
}

impl AsMut<[u8]> for SliceableMemoryMap {
    fn as_mut(&mut self) -> &mut [u8] { self.as_mut_slice() }
}

/// Direction for the region search.
pub enum RegionSearch {
    Before,
    After,
}

/// An iterator searching for free regions.
pub struct RegionFreeIter {
    range: Range<usize>,
    search: RegionSearch,
    current: usize,
}

impl RegionFreeIter {
    /// Creates a new iterator for free regions.
    pub fn new(origin: *const (), range: Option<Range<usize>>, search: RegionSearch) -> Self {
        RegionFreeIter {
            range: range.unwrap_or(0..usize::max_value()),
            current: origin as usize,
            search: search,
        }
    }
}

impl Iterator for RegionFreeIter {
    type Item = Result<*const ()>;

    /// Returns the next free region for the current address.
    fn next(&mut self) -> Option<Self::Item> {
        let page_size = region::page_size();

        while self.current > 0 && self.range.contains(self.current) {
            match region::query(self.current as *const _) {
                Ok(region) => self.current = match self.search {
                    RegionSearch::Before => region.lower().saturating_sub(page_size),
                    RegionSearch::After => region.upper(),
                },
                Err(error) => {
                    match self.search {
                        RegionSearch::Before => self.current -= page_size,
                        RegionSearch::After => self.current += page_size,
                    }

                    // Check whether the region is free, otherwise return the error
                    return Some(matches!(error.kind(), &region::error::ErrorKind::Freed)
                        .as_result(self.current as *const _, error.into()));
                },
            }
        }

        None
    }
}
