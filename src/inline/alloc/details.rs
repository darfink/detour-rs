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
pub type AllocType = PoolVal<u8>;

/// Shared instance containing all pools
pub struct Allocator {
    pub max_distance: usize,
    pub pools: Vec<SlicePool<u8>>,
}

// TODO: Search backwards for regions as well
impl Allocator {
    /// Releases the associated memory pool, if it contains no more references.
    pub fn release(&mut self, value: &AllocType) {
    }

    /// Allocates a slice in an eligible memory map.
    pub fn allocate(&mut self, origin: *const (), size: usize) -> Result<AllocType> {
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

    /// Allocates a chunk using any of the existing pools.
    fn allocate_existing(&mut self, range: &Range<usize>, size: usize) -> Option<AllocType> {
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
        while let Some(address) = Self::find_free_region(origin as *const (), range)? {
            if let Some(pool) = Self::allocate_region_pool(address, size) {
                return Ok(pool);
            }
        }

        bail!(ErrorKind::OutOfMemory);
    }

    /// Returns the closest free region for the specified address.
    fn find_free_region(origin: *const (), range: &Range<usize>) -> Result<Option<*const ()>> {
        let mut target = origin as *const u8;

        while range.contains(target as usize) {
            match region::query(target) {
                // This chunk is occupied, so advance to the next region
                Ok(region) => target = region.upper() as *const u8,
                Err(error) => return match error.kind() {
                    &region::error::ErrorKind::Freed => Ok(Some(target as *const _)),
                    _ => Err(error.into()),
                }
            }
        }

        Ok(None)
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
