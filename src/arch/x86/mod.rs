pub use self::patcher::Patcher;
pub use self::trampoline::Trampoline;

pub mod meta;
mod patcher;
mod thunk;
mod trampoline;

// TODO: Add test for targets further away than DETOUR_RANGE
// TODO: Add test for unsupported branches
// TODO: Add test for negative branch displacements
#[cfg(all(feature = "nightly", test))]
mod tests {
  use error::*;
  use std::mem;
  use RawDetour;

  /// Detours a C function returning an integer, and asserts its return value.
  #[inline(never)]
  unsafe fn detour_test(target: funcs::CRet, result: i32) {
    let mut hook = RawDetour::new(target as *const (), funcs::ret10 as *const ()).unwrap();

    assert_eq!(target(), result);
    hook.enable().unwrap();
    {
      assert_eq!(target(), 10);

      let original: funcs::CRet = mem::transmute(hook.trampoline());
      assert_eq!(original(), result);
    }
    hook.disable().unwrap();
    assert_eq!(target(), result);
  }

  #[test]
  fn detour_relative_branch() {
    unsafe {
      detour_test(mem::transmute(funcs::branch_ret5 as usize), 5);
    }
  }

  #[test]
  fn detour_hotpatch() {
    unsafe {
      detour_test(mem::transmute(funcs::hotpatch_ret0 as usize + 5), 0);
    }
  }

  #[test]
  fn detour_padding_after() {
    unsafe {
      detour_test(mem::transmute(funcs::padding_after_ret0 as usize + 2), 0);
    }
  }

  #[test]
  fn detour_external_loop() {
    let error =
      unsafe { RawDetour::new(funcs::external_loop as *const (), funcs::ret10 as *const ()) }
        .unwrap_err();
    assert_matches!(error, Error::UnsupportedInstruction);
  }

  #[test]
  #[cfg(target_arch = "x86_64")]
  fn detour_rip_relative_pos() {
    unsafe {
      detour_test(funcs::rip_relative_ret195, 195);
    }
  }

  #[test]
  #[cfg(target_arch = "x86_64")]
  fn detour_rip_relative_neg() {
    unsafe {
      detour_test(funcs::rip_relative_prolog_ret49, 49);
    }
  }

  /// Case specific functions.
  mod funcs {
    pub type CRet = unsafe extern "C" fn() -> i32;

    #[naked]
    #[inline(never)]
    pub unsafe extern "C" fn branch_ret5() -> i32 {
      asm!("xor eax, eax
                  je ret5
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
                  ret
                  mov eax, 5"
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
    pub unsafe extern "C" fn rip_relative_prolog_ret49() -> i32 {
      asm!("xor eax, eax
                  mov al, [rip-0x8]
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
