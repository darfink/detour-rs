use std::mem;
use region;
use error::*;
use util;
use Detour;

pub struct Virtual {
    enabled: bool,
    vtable: *const *const (),
    detour: *const (),
    original: *const (),
    region: region::View,
    index: usize,
}

impl Virtual {
    /// Constructs a new virtual detour from an object's virtual table.
    pub unsafe fn new<T>(object: &T, index: usize, detour: *const ()) -> Result<Self> {
        Self::with_table(*mem::transmute::<&T, *const *const *const ()>(object), index, detour)
    }

    /// Constructs a new virtual detour directly from a virtual table.
    pub unsafe fn with_table(vtable: *const *const (), index: usize, detour: *const ()) -> Result<Self> {
        let entry = vtable.offset(index as isize);
        let view = region::View::new(entry as *const _, mem::size_of::<usize>())?;

        // The virtual table should only have read access.
        if view.get_prot() == Some(region::Protection::Read) {
            bail!(ErrorKind::IsExecutable);
        }

        // The function address at the specified index should be executable.
        if !util::is_executable_address(*entry)? || !util::is_executable_address(detour)? {
            bail!(ErrorKind::NotExecutable);
        }

        Ok(Virtual {
            enabled: false,
            vtable: vtable,
            detour: detour,
            original: *entry,
            region: view,
            index: index,
        })
    }

    /// Toggles the state of the virtual detour.
    unsafe fn toggle(&mut self, enable: bool) -> Result<()> {
        if self.enabled == enable {
            return Ok(());
        }

        let offset = (self.vtable as usize + self.index) as *mut *const ();
        let replacement = if enable { self.detour } else { self.original };

        self.region.exec_with_prot(region::Protection::ReadWrite, || *offset = replacement)?;
        self.enabled = enable;
        Ok(())
    }
}

impl Detour for Virtual {
    unsafe fn enable(&mut self) -> Result<()> {
        self.toggle(true)
    }

    unsafe fn disable(&mut self) -> Result<()> {
        self.toggle(false)
    }

    fn callable_address(&self) -> *const () {
        self.original
    }

    fn is_hooked(&self) -> bool {
        self.enabled
    }
}

impl Drop for Virtual {
    /// Removes the virtual method hook.
    fn drop(&mut self) {
        unsafe { self.disable().unwrap() };
    }
}
