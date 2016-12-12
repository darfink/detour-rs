use std::sync::Mutex;
use error::*;
use inline::pic;
use {util, Detour};
use super::{arch, alloc};

lazy_static! {
    /// Mutex ensuring only one thread detours at a time.
    static ref POOL: Mutex<alloc::ProximityAllocator> = {
        // Use a range of +/- 512 MB for seeking a memory block
        Mutex::new(alloc::ProximityAllocator::new(0x20000000))
    };
}

pub struct InlineDetour {
    patcher: arch::Patcher,
    trampoline: alloc::ProximitySlice,
}

// TODO: stop all threads in target during patch?
// TODO: add relay function for x64
impl InlineDetour {
    /// Constructs a new inline detour patcher.
    pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
        let mut pool = POOL.lock().unwrap();
        if !util::is_executable_address(target)? || !util::is_executable_address(detour)? {
            bail!(ErrorKind::NotExecutable);
        }

        // Create a trampoline generator for the target function
        let patch_size = arch::Patcher::default_patch_size(target);
        let trampoline = arch::Trampoline::new(target, patch_size)?;

        Ok(InlineDetour {
            patcher: arch::Patcher::new(target, detour, trampoline.prolog_size())?,
            trampoline: Self::create_trampoline(&mut pool, trampoline.builder(), target)?,
        })
    }

    /// Creates a trampoline for a specific address.
    pub fn create_trampoline(pool: &mut alloc::ProximityAllocator,
                             builder: &pic::CodeBuilder,
                             target: *const ()) -> Result<alloc::ProximitySlice> {
        // Allocate memory close to the target
        let mut memory = pool.allocate(target, builder.len())?;

        // Generate code for the newly allocated address
        let code = builder.build(memory.as_ptr() as *const _);
        memory.copy_from_slice(code.as_slice());
        Ok(memory)
    }
}

impl Detour for InlineDetour {
    unsafe fn enable(&mut self) -> Result<()> {
        let _guard = POOL.lock().unwrap();
        self.patcher.toggle(true)
    }

    unsafe fn disable(&mut self) -> Result<()> {
        let _guard = POOL.lock().unwrap();
        self.patcher.toggle(false)
    }

    fn callable_address(&self) -> *const () {
        self.trampoline.as_ptr() as *const ()
    }

    fn is_hooked(&self) -> bool {
        self.patcher.is_patched()
    }
}

impl Drop for InlineDetour {
    fn drop(&mut self) {
        unsafe { self.disable().unwrap() };
    }
}

