use std::fmt;
use std::sync::Mutex;

use error::*;
use {pic, util, arch, alloc};

lazy_static! {
    /// Shared allocator for all detours.
    static ref POOL: Mutex<alloc::Allocator> = {
        // Use a range of +/- 2 GB for seeking a memory block
        Mutex::new(alloc::Allocator::new(0x80000000))
    };
}

/// Implementation of an inline detour.
pub struct InlineDetour {
    patcher: arch::Patcher,
    trampoline: alloc::Slice,
    #[allow(dead_code)]
    relay: Option<alloc::Slice>,
}

// TODO: stop all threads in target during patch?
impl InlineDetour {
    /// Constructs a new inline detour patcher.
    pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
        let mut pool = POOL.lock().unwrap();
        if !util::is_executable_address(target)? || !util::is_executable_address(detour)? {
            bail!(ErrorKind::NotExecutable);
        }

        // Create a trampoline generator for the target function
        let margin = arch::Patcher::prolog_margin(target);
        let trampoline = arch::Trampoline::new(target, margin)?;

        // A relay is used in case a normal branch cannot reach the destination
        let relay = if let Some(emitter) = arch::relay_builder(target, detour)? {
            Some(Self::allocate_code(&mut pool, &emitter, target)?)
        } else {
            None
        };

        // If a relay is supplied, use it instead of the detour address
        let detour = relay.as_ref().map(|code| code.as_ptr() as *const ()).unwrap_or(detour);

        Ok(InlineDetour {
            patcher: arch::Patcher::new(target, detour, trampoline.prolog_size())?,
            trampoline: Self::allocate_code(&mut pool, trampoline.emitter(), target)?,
            relay,
        })
    }

    /// Enables the detour.
    pub unsafe fn enable(&mut self) -> Result<()> {
        let _guard = POOL.lock().unwrap();
        self.patcher.toggle(true)
    }

    /// Disables the detour.
    pub unsafe fn disable(&mut self) -> Result<()> {
        let _guard = POOL.lock().unwrap();
        self.patcher.toggle(false)
    }

    /// Returns a callable address to the target.
    pub fn callable_address(&self) -> *const () {
        self.trampoline.as_ptr() as *const ()
    }

    /// Returns whether the target is hooked or not.
    pub fn is_enabled(&self) -> bool {
        self.patcher.is_patched()
    }

    /// Allocates PIC code at the specified address.
    fn allocate_code(pool: &mut alloc::Allocator,
                     emitter: &pic::CodeEmitter,
                     origin: *const ()) -> Result<alloc::Slice> {
        // Allocate memory close to the origin
        let mut memory = pool.allocate(origin, emitter.len())?;

        // Generate code for the obtained address
        let code = emitter.emit(memory.as_ptr() as *const _);
        memory.copy_from_slice(code.as_slice());
        Ok(memory)
    }
}

impl Drop for InlineDetour {
    /// Disables the detour, if enabled.
    fn drop(&mut self) {
        unsafe { self.disable().unwrap() };
    }
}

impl fmt::Debug for InlineDetour {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "InlineDetour {{ enabled: {}, trampoline: {:?} }}",
               self.is_enabled(), self.callable_address())
    }
}
