use std::io::Write;
use std::sync::Mutex;

use memmap::{Mmap, Protection};

use Detour;
use super::arch;
use inline::pic;
use error::*;
use util;

lazy_static! {
    /// Mutex ensuring only one thread detours at a time.
    static ref LOCK: Mutex<bool> = Mutex::new(false);
}

pub struct InlineDetour {
    patcher: arch::Patcher,
    trampoline: Mmap,
}

// TODO: stop all threads in target during patch?
impl InlineDetour {
    /// Constructs a new inline detour patcher.
    pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
        let _guard = LOCK.lock().unwrap();
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

    // TODO: allocate the trampoline close to the target
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
        let _guard = LOCK.lock().unwrap();
        self.patcher.toggle(true)
    }

    unsafe fn disable(&mut self) -> Result<()> {
        let _guard = LOCK.lock().unwrap();
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

    /// Detours a simple C function and asserts all results.
    unsafe fn detour_test(target: funcs::CRet, result: i32) {
        let mut hook = InlineDetour::new(target as *const (), funcs::ret10 as *const ()).unwrap();

        assert_eq!(target(), result);
        hook.enable().unwrap();
        {
            assert_eq!(target(), 10);

            let original: funcs::CRet = mem::transmute(hook.callable_address());
            assert_eq!(original(), result);
        }
        hook.disable().unwrap();
        assert_eq!(target(), result);
    }

    #[test]
    fn detour_relative_branch() {
        unsafe { detour_test(funcs::branch_ret5, 5); }
    }

    #[test]
    fn detour_hot_patch() {
        unsafe { detour_test(mem::transmute(funcs::hotpatch_ret0 as usize + 5), 0); }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn detour_rip_relative() {
        unsafe { detour_test(funcs::rip_relative_ret195, 195); }
    }

    /// Exports assembly specific functions.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    mod funcs {
        pub type CRet = unsafe extern "C" fn() -> i32;

        #[naked]
        pub unsafe extern "C" fn branch_ret5() -> i32 {
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
        pub unsafe extern "C" fn hotpatch_ret0() -> i32 {
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

        //#[naked]
        //pub unsafe extern "C" fn hotpatch_after_ret0() -> i32 {
        //    asm!("mov edi, edi
        //          xor eax, eax
        //          ret
        //          nop
        //          nop
        //          nop
        //          nop"
        //          :::: "intel");
        //    ::std::intrinsics::unreachable();
        //}

        #[naked]
        #[cfg(target_arch = "x86_64")]
        pub unsafe extern "C" fn rip_relative_ret195() -> i32 {
            asm!("xor eax, eax
                  mov al, [rip+0x3]
                  nop
                  nop
                  nop
                  ret"
                 :::: "intel");
            ::std::intrinsics::unreachable();
        }

        /// The default detour target.
        pub unsafe extern "C" fn ret10() -> i32 {
            10
        }
    }
}
