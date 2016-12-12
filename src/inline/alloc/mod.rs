use std::ops::{Deref, DerefMut};
use std::cell::RefCell;
use std::rc::Rc;
use error::*;

mod details;

/// A memory pool for allocating chunks close to addresses.
pub struct ProximityAllocator(Rc<RefCell<details::Allocator>>);

impl ProximityAllocator {
    /// Creates a new proximity allocator
    pub fn new(max_distance: usize) -> Self {
        ProximityAllocator(Rc::new(RefCell::new(details::Allocator {
            max_distance: max_distance,
            pools: Vec::new(),
        })))
    }

    /// Allocates a new slice close to `origin`.
    pub fn allocate(&mut self, origin: *const (), size: usize) -> Result<ProximitySlice> {
        let mut allocator = self.0.borrow_mut();
        allocator.allocate(origin, size).map(|value| ProximitySlice {
            allocator: self.0.clone(),
            value: value,
        })
    }
}

/// Safe to use with a `Mutex`.
unsafe impl Send for ProximityAllocator { }

/// A handle for allocated proximity memory.
pub struct ProximitySlice {
    allocator: Rc<RefCell<details::Allocator>>,
    value: details::AllocType,
}

impl Drop for ProximitySlice {
    fn drop(&mut self) {
        // This call may free the associated memory map
        self.allocator.borrow_mut().release(&self.value);
    }
}

impl Deref for ProximitySlice {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.value.deref()
    }
}

impl DerefMut for ProximitySlice {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.value.deref_mut()
    }
}
