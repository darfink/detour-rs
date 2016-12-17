extern crate libudis86_sys as udis;
use std::mem;
use error::*;
use pic;

// Re-exports
pub use self::patcher::Patcher;
pub use self::trampoline::Trampoline;

// Modules
mod patcher;
mod trampoline;
mod thunk;

/// Creates a relay. Required for destinations further away than 2GB.
pub unsafe fn relay_builder(destination: *const ()) -> Result<Option<pic::CodeBuilder>> {
    if cfg!(target_arch = "x86_64") {
        let mut builder = pic::CodeBuilder::new();
        builder.add_thunk(thunk::jmp(mem::transmute(destination)));
        Ok(Some(builder))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::mem;
    use InlineDetour;
    use error::*;

    /// Detours a C function returning an integer, and asserts all results.
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
    fn detour_padding_after() {
        unsafe { detour_test(mem::transmute(funcs::padding_after_ret0 as usize + 2), 0); }
    }

    #[test]
    fn detour_external_loop() {
        unsafe {
            let error = InlineDetour::new(funcs::external_loop as *const (),
                                          funcs::ret10 as *const ()).unwrap_err();
            assert!(matches!(error.kind(), &ErrorKind::ExternalLoop));
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn detour_rip_relative() {
        unsafe { detour_test(funcs::rip_relative_ret195, 195); }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn detour_rip_relative_neg() {
        unsafe { detour_test(funcs::rip_relative_neg_ret49, 49); }
    }

    /// Case specific functions.
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

        #[naked]
        pub unsafe extern "C" fn padding_after_ret0() -> i32 {
            asm!("mov edi, edi
                  xor eax, eax
                  ret
                  nop
                  nop"
                 :::: "intel");
            ::std::intrinsics::unreachable();
        }

        #[naked]
        pub unsafe extern "C" fn external_loop() -> i32 {
            asm!("loop dest
                  nop
                  nop
                  nop
                dest:"
                 :::: "intel");
            ::std::intrinsics::unreachable();
        }

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

        #[naked]
        #[cfg(target_arch = "x86_64")]
        pub unsafe extern "C" fn rip_relative_neg_ret49() -> i32 {
            asm!("xor eax, eax
                  mov al, [rip-0x8]
                  nop
                  nop
                  nop
                  ret"
                 :::: "intel");
            ::std::intrinsics::unreachable();
        }

        /// Default detour target.
        pub unsafe extern "C" fn ret10() -> i32 {
            10
        }
    }
}
