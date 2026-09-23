//! Architecture-specific tests, using hand-written function prologs.
use detour::{RawDetour, Result};
use std::mem;

/// Default test case function definition.
type CRet = unsafe extern "C" fn() -> i32;

/// The default detour.
extern "C" fn ret10() -> i32 {
  10
}

/// Detours `target`, and asserts its return value before, during, and after.
unsafe fn detour_test(target: CRet, result: i32) -> Result<()> {
  // SAFETY: Both functions share the same signature.
  let hook = unsafe { RawDetour::new(target as *const (), ret10 as *const ())? };

  // SAFETY: The fixtures are valid functions, and not executed concurrently.
  unsafe {
    assert_eq!(target(), result);
    hook.enable()?;
    {
      assert_eq!(target(), 10);
      let original: CRet = mem::transmute(hook.trampoline());
      assert_eq!(original(), result);
    }
    hook.disable()?;
    assert_eq!(target(), result);
  }
  Ok(())
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
  use super::*;
  use detour::Error;
  use std::arch::naked_asm;

  /// Offsets a function pointer.
  fn offset(function: CRet, bytes: usize) -> CRet {
    // SAFETY: Used for fixtures with a valid entry point at the offset.
    unsafe { mem::transmute(function as usize + bytes) }
  }

  #[test]
  fn relative_branch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn branch_ret5() -> i32 {
      naked_asm!(
        "xor eax, eax",
        "je 2f",
        "mov eax, 2",
        "jmp 3f",
        "2:",
        "mov eax, 5",
        "3:",
        "ret",
      )
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(branch_ret5, 5) }
  }

  #[test]
  fn internal_branch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn internal_ret42() -> i32 {
      naked_asm!("xor eax, eax", "jz 2f", "2:", "mov al, 42", "ret",)
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(internal_ret42, 42) }
  }

  #[test]
  fn external_conditional_branch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn jcc_ret9() -> i32 {
      naked_asm!(
        "xor eax, eax",
        // A short jcc leaving the prolog, which must be widened
        "jz 2f",
        "mov eax, 1",
        "ret",
        "nop",
        "nop",
        "nop",
        "nop",
        "2:",
        "mov eax, 9",
        "ret",
      )
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(jcc_ret9, 9) }
  }

  #[test]
  fn relative_call() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn call_ret42() -> i32 {
      naked_asm!("call 2f", "add eax, 1", "ret", "2:", "mov eax, 41", "ret",)
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(call_ret42, 42) }
  }

  #[test]
  fn internal_branch_into_instruction() {
    #[unsafe(naked)]
    unsafe extern "C" fn misaligned_branch() -> i32 {
      naked_asm!(
        "xor eax, eax",
        // jz +1 (into the immediate of the following instruction)
        ".byte 0x74, 0x01",
        "mov eax, 0x90909090",
        "ret",
      )
    }

    // SAFETY: The detour is never enabled.
    let error =
      unsafe { RawDetour::new(misaligned_branch as *const (), ret10 as *const ()) }.unwrap_err();
    assert!(matches!(error, Error::UnsupportedInstruction));
  }

  #[test]
  fn hot_patch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn hotpatch_ret0() -> i32 {
      naked_asm!(
        "nop",
        "nop",
        "nop",
        "nop",
        "nop",
        "xor eax, eax",
        "ret",
        "mov eax, 5",
      )
    }

    // SAFETY: The function is preceded by a hot-patch area.
    unsafe { detour_test(offset(hotpatch_ret0, 5), 0) }
  }

  #[test]
  fn padding_after() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn padding_after_ret0() -> i32 {
      naked_asm!("mov edi, edi", "xor eax, eax", "ret", "nop", "nop",)
    }

    // SAFETY: The function is followed by padding.
    unsafe { detour_test(offset(padding_after_ret0, 2), 0) }
  }

  #[test]
  fn multi_byte_nop_padding() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn padding_after_ret1() -> i32 {
      naked_asm!(
        "mov edi, edi",
        "mov al, 1",
        "ret",
        // nop dword ptr [eax]
        ".byte 0x0F, 0x1F, 0x00",
      )
    }

    // SAFETY: The function is followed by padding.
    unsafe {
      let target = offset(padding_after_ret1, 2);
      // Only the lower byte is set by the fixture
      let hook = RawDetour::new(target as *const (), ret10 as *const ())?;
      hook.enable()?;
      assert_eq!(target(), 10);
      let original: CRet = mem::transmute(hook.trampoline());
      assert_eq!(original() & 0xFF, 1);
    }
    Ok(())
  }

  #[test]
  fn no_patch_area() {
    #[unsafe(naked)]
    unsafe extern "C" fn tiny() -> i32 {
      naked_asm!("mov edi, edi", "xor eax, eax", "ret", "mov eax, 1", "ret")
    }

    // SAFETY: The detour is never enabled.
    let error =
      unsafe { RawDetour::new(offset(tiny, 2) as *const (), ret10 as *const ()) }.unwrap_err();
    assert!(matches!(error, Error::PatchAreaTooSmall));
  }

  #[test]
  fn external_loop() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn loop_ret7() -> i32 {
      naked_asm!(
        "xor ecx, ecx",
        "mov cl, 2",
        // Taken, since ecx is non-zero after the decrement
        "loop 2f",
        "mov eax, 5",
        "ret",
        "nop",
        "nop",
        "nop",
        "2:",
        "mov eax, 7",
        "ret",
      )
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(loop_ret7, 7) }
  }

  #[test]
  #[cfg(target_arch = "x86_64")]
  fn rip_relative() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn rip_relative_ret195() -> i32 {
      naked_asm!(
        "xor eax, eax",
        "mov al, [rip + 3]",
        "nop",
        "nop",
        "nop",
        "ret",
      )
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(rip_relative_ret195, 195) }
  }

  #[test]
  #[cfg(target_arch = "x86_64")]
  fn rip_relative_into_patch_area() {
    #[unsafe(naked)]
    unsafe extern "C" fn rip_relative_prolog_ret49() -> i32 {
      naked_asm!("xor eax, eax", "mov al, [rip - 8]", "ret")
    }

    // The relocated instruction would observe the patched jump
    // SAFETY: The detour is never enabled.
    let error =
      unsafe { RawDetour::new(rip_relative_prolog_ret49 as *const (), ret10 as *const ()) }
        .unwrap_err();
    assert!(matches!(error, Error::UnsupportedInstruction));
  }

  #[test]
  #[cfg(target_arch = "x86")]
  fn position_independent_call() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn get_pc() -> i32 {
      naked_asm!("call 2f", "2:", "pop eax", "ret")
    }

    // The trampoline must push the original return address
    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(get_pc, get_pc as *const () as usize as i32 + 5) }
  }
}

#[cfg(target_arch = "aarch64")]
mod aarch64 {
  use super::*;
  use std::arch::naked_asm;

  type CRetArg = unsafe extern "C" fn(i64) -> i32;

  /// Detours `target`, and asserts its results for `inputs`.
  unsafe fn detour_test_arg(target: CRetArg, cases: &[(i64, i32)]) -> Result<()> {
    extern "C" fn detour(_: i64) -> i32 {
      10
    }

    // SAFETY: Both functions share the same signature.
    let hook = unsafe { RawDetour::new(target as *const (), detour as *const ())? };
    // SAFETY: The fixtures are valid functions, and not executed concurrently.
    unsafe {
      hook.enable()?;
      let original: CRetArg = mem::transmute(hook.trampoline());
      for &(input, output) in cases {
        assert_eq!(target(input), 10);
        assert_eq!(original(input), output);
      }
      hook.disable()?;
      for &(input, output) in cases {
        assert_eq!(target(input), output);
      }
    }
    Ok(())
  }

  #[test]
  fn adr() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn adr_ret42() -> i32 {
      naked_asm!("adr x0, 2f", "ldr w0, [x0]", "ret", "2:", ".word 42")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(adr_ret42, 42) }
  }

  #[test]
  fn adrp() -> Result<()> {
    static VALUE: i32 = 1337;

    #[unsafe(naked)]
    unsafe extern "C" fn adrp_ret1337() -> i32 {
      #[cfg(target_vendor = "apple")]
      naked_asm!(
        "adrp x0, {value}@PAGE",
        "add x0, x0, {value}@PAGEOFF",
        "ldr w0, [x0]",
        "ret",
        value = sym VALUE,
      );
      #[cfg(not(target_vendor = "apple"))]
      naked_asm!(
        "adrp x0, {value}",
        "add x0, x0, :lo12:{value}",
        "ldr w0, [x0]",
        "ret",
        value = sym VALUE,
      );
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(adrp_ret1337, 1337) }
  }

  #[test]
  fn literal_load() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn literal_ret77() -> i32 {
      naked_asm!("ldr w0, 2f", "ret", "2:", ".word 77")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(literal_ret77, 77) }
  }

  #[test]
  fn literal_load_simd() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn literal_ret3() -> i32 {
      naked_asm!(
        "ldr d0, 2f",
        "fcvtzs w0, d0",
        "ret",
        ".p2align 3",
        "2:",
        ".double 3.0"
      )
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(literal_ret3, 3) }
  }

  #[test]
  fn literal_load_64bit() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn literal_ret_high() -> i64 {
      naked_asm!("ldr x0, 2f", "ret", ".p2align 3", "2:", ".quad 0x123456789")
    }

    extern "C" fn detour() -> i64 {
      0
    }

    // SAFETY: Both functions share the same signature.
    let hook = unsafe { RawDetour::new(literal_ret_high as *const (), detour as *const ())? };
    // SAFETY: The fixture is not executed concurrently.
    unsafe {
      hook.enable()?;
      assert_eq!(literal_ret_high(), 0);
      let original: unsafe extern "C" fn() -> i64 = mem::transmute(hook.trampoline());
      assert_eq!(original(), 0x1_2345_6789);
    }
    Ok(())
  }

  #[test]
  fn branch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn branch_ret2() -> i32 {
      naked_asm!("b 2f", "mov w0, #1", "ret", "2:", "mov w0, #2", "ret")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(branch_ret2, 2) }
  }

  #[test]
  fn compare_and_branch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn is_zero(_: i64) -> i32 {
      naked_asm!("cbz x0, 2f", "mov w0, #1", "ret", "2:", "mov w0, #2", "ret")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test_arg(is_zero, &[(0, 2), (5, 1)]) }
  }

  #[test]
  fn test_and_branch() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn is_even(_: i64) -> i32 {
      naked_asm!(
        "tbz w0, #0, 2f",
        "mov w0, #1",
        "ret",
        "2:",
        "mov w0, #2",
        "ret"
      )
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test_arg(is_even, &[(4, 2), (7, 1)]) }
  }

  #[test]
  fn conditional_branch() -> Result<()> {
    // The condition flags are set by the caller
    #[unsafe(naked)]
    unsafe extern "C" fn is_negative_inner() -> i32 {
      naked_asm!("b.mi 2f", "mov w0, #1", "ret", "2:", "mov w0, #2", "ret")
    }

    extern "C" fn detour() -> i32 {
      10
    }

    /// Sets the condition flags, and tail-calls `function`.
    macro_rules! call_with_flags {
      ($function:expr, $value:expr) => {{
        let result: i32;
        // SAFETY: The function preserves the stack, and clobbers x0 only.
        unsafe {
          std::arch::asm!(
            "cmp {value}, #0",
            "blr {function}",
            value = in(reg) $value as i64,
            function = in(reg) $function,
            out("x0") result,
            out("x30") _,
            clobber_abi("C"),
          )
        };
        result
      }};
    }

    // SAFETY: Both functions share the same signature.
    let hook = unsafe { RawDetour::new(is_negative_inner as *const (), detour as *const ())? };
    assert_eq!(call_with_flags!(is_negative_inner, -5), 2);
    // SAFETY: The fixture is not executed concurrently.
    unsafe { hook.enable()? };
    assert_eq!(call_with_flags!(is_negative_inner, -5), 10);
    assert_eq!(call_with_flags!(hook.trampoline(), -5), 2);
    assert_eq!(call_with_flags!(hook.trampoline(), 5), 1);
    Ok(())
  }

  #[test]
  fn branch_target_identification() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn bti_ret5() -> i32 {
      // bti c; mov w0, #5; ret
      naked_asm!("hint #34", "mov w0, #5", "ret")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(bti_ret5, 5) }?;

    // The landing pad must remain at the entry point
    // SAFETY: The instruction is readable.
    let entry = unsafe { (bti_ret5 as *const u32).read() };
    assert_eq!(entry, 0xD503_245F);
    Ok(())
  }

  #[test]
  fn pointer_authentication() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn pac_ret5() -> i32 {
      // paciasp; mov w0, #5; autiasp; ret
      naked_asm!("hint #25", "mov w0, #5", "hint #29", "ret")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test(pac_ret5, 5) }
  }

  #[test]
  fn single_instruction_function() -> Result<()> {
    #[unsafe(naked)]
    unsafe extern "C" fn ret_arg(_: i64) -> i32 {
      naked_asm!("ret")
    }

    // SAFETY: The fixture is a valid function.
    unsafe { detour_test_arg(ret_arg, &[(3, 3), (4, 4)]) }
  }
}
