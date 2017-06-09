use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};
use error::*;

mod details;

/// A thread-safe memory pool for allocating chunks close to addresses.
pub struct ProximityAllocator(Arc<Mutex<details::Allocator>>);

impl ProximityAllocator {
    /// Creates a new proximity allocator
    pub fn new(max_distance: usize) -> Self {
        ProximityAllocator(Arc::new(Mutex::new(details::Allocator {
            max_distance: max_distance,
            pools: Vec::new(),
        })))
    }

    /// Allocates a new slice close to `origin`.
    pub fn allocate(&mut self, origin: *const (), size: usize) -> Result<Slice> {
        let mut allocator = self.0.lock().unwrap();
        allocator.allocate(origin, size).map(|value| Slice {
            allocator: self.0.clone(),
            value: value,
        })
    }
}

// TODO: Come up with a better name
/// A handle for allocated proximity memory.
pub struct Slice {
    allocator: Arc<Mutex<details::Allocator>>,
    value: details::Allocation,
}

impl Drop for Slice {
    fn drop(&mut self) {
        // Release the associated memory map (if unique)
        self.allocator.lock().unwrap().release(&self.value);
    }
}

impl Deref for Slice {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.value.deref()
    }
}

impl DerefMut for Slice {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.value.deref_mut()
    }
}
