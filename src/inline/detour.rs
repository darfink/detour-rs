use std::io::Write;

use memmap::{Mmap, Protection};

use Detour;
use super::arch;
use inline::pic;
use error::*;
use util;

pub struct InlineDetour {
    patcher: arch::Patcher,
    trampoline: Mmap,
}

impl InlineDetour {
    /// Constructs a new inline detour patcher.
    pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
        if !util::is_executable_address(target)? || !util::is_executable_address(detour)? {
            bail!(ErrorKind::NotExecutable);
        }

        // Create a trampoline generator for the target function
        let patch_size = arch::Patcher::patch_size(target);
        let trampoline = arch::Trampoline::new(target, patch_size)?;

        Ok(InlineDetour {
            patcher: arch::Patcher::new(target, detour, trampoline.prolog_size())?,
            trampoline: Self::allocate_trampoline(trampoline.generator())?,
        })
    }

    // TODO: allocate a trampoline close to the target
    fn allocate_trampoline(generator: &pic::Generator) -> Result<Mmap> {
        // Create a memory map for the trampoline
        let mut map = Mmap::anonymous(generator.len(), Protection::ReadWrite)?;

        // Generate the raw instruction bytes for the specific address
        let trampoline = generator.generate(map.ptr() as *const ());
        unsafe { map.as_mut_slice().write(&trampoline)? };
        map.set_protection(Protection::ReadExecute)?;
        Ok(map)
    }
}

impl Detour for InlineDetour {
    unsafe fn enable(&mut self) -> Result<()> {
        self.patcher.toggle(true)
    }

    unsafe fn disable(&mut self) -> Result<()> {
        self.patcher.toggle(false)
    }

    fn callable_address(&self) -> *const () {
        self.trampoline.ptr() as *const ()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    type CRet = unsafe extern "C" fn() -> i32;

    #[naked]
    unsafe extern "C" fn branch_ret5() -> i32 {
        asm!("test sp, sp
              jne ret5
              mov eax, 2
              jmp done
            ret5:
              mov eax, 5
            done:
              ret"
             :::: "intel");
        ::std::intrinsics::unreachable();
    }

    #[naked]
    unsafe extern "C" fn hotpatch_ret0() -> i32 {
        asm!("nop
              nop
              nop
              nop
              nop
              xor eax, eax
              ret"
             :::: "intel");
        ::std::intrinsics::unreachable();
    }

    unsafe extern "C" fn ret10() -> i32 {
        10
    }

	unsafe fn detour_test(target: CRet, result: i32) {
		let mut hook = InlineDetour::new(target as *const (), ret10 as *const ()).unwrap();

		assert_eq!(target(), result);
		hook.enable().unwrap();
		{
			assert_eq!(target(), 10);

			let original: CRet = mem::transmute(hook.callable_address());
			assert_eq!(original(), result);
		}
		hook.disable().unwrap();
		assert_eq!(target(), result);
	}

    #[test]
    fn detour_relative_branch() {
        unsafe { detour_test(branch_ret5, 5); }
    }

    #[test]
    fn detour_hot_patch() {
        unsafe { detour_test(mem::transmute(hotpatch_ret0 as usize + 5), 0); }
    }
}
