#![recursion_limit = "1024"]
#![feature(range_contains, offset_to)]
#![cfg_attr(test, feature(naked_functions, core_intrinsics, asm))]
#![cfg_attr(feature = "static", feature(
    const_fn,
    const_ptr_null_mut,
    const_atomic_ptr_new,
    unboxed_closures,
))]

//! A cross-platform detour library written in Rust.
//!
//! ## Intro
//!
//! This library provides a thread-safe, inline detouring functionality by
//! disassembling and patching functions during runtime, using assembly opcodes
//! allocated within executable memory. It modifies the target functions and
//! replaces their prolog with an unconditional jump.
//!
//! Beyond the basic functionality this library handles several different edge
//! cases:
//!
//! - Relative branches.
//! - RIP relative operands.
//! - Detects NOP-padding.
//! - Relay for large offsets (>2GB)
//! - Supports hot patching.
//!
//! ## Detours
//!
//! Three different types of detours are provided:
//!
//! - [Generic](./struct.GenericDetour.html): A type-safe interface — the same
//!   prototype is enforced for both the target and the detour.  
//!   It is also enforced when invoking the original target.
//!
//! - [Static](./struct.StaticDetour.html): A static & type-safe interface.
//!   Thanks to its static nature it can accept a closure as its second
//!   argument, but on the other hand, it can only have one detour active at a
//!   time.
//!
//! - [Raw](./struct.RawDetour.html): The underlying building block that
//!   the others types abstract upon. It has no type-safety and interacts with
//!   raw pointers.  
//!   It should be avoided unless the types used aren't known until runtime.
//!
//! All detours implement the [Detour](./trait.Detour.html) trait, which exposes
//! several methods, and enforces `Send + Sync`. Therefore you must also include
//! it into your scope whenever you are using a detour.
//!
//! ## Features
//!
//! - **static**: Enabled by default. Includes the static detour functionality,
//!   but requires the nightly features *const_fn* & *unboxed_closures*.
//!
//! ## Procedure
//!
//! To illustrate on an x86 platform:
//!
//! ```c
//! 0 int return_five() {
//! 1     return 5;
//! 00400020 [b8 05 00 00 00] mov eax, 5
//! 00400025 [c3]             ret
//! 2 }
//! 3
//! 4 int detour_function() {
//! 5     return 10;
//! 00400040 [b8 0A 00 00 00] mov eax, 10
//! 00400045 [c3]             ret
//! 6 }
//! ```
//!
//! To detour `return_five` the library by default tries to replace five bytes
//! with a relative jump (the optimal scenario), which works in this case.
//! Executable memory will be allocated for the instruction and the function's
//! prolog will be replaced.
//!
//! ```c
//! 0 int return_five() {
//! 1     return detour_function();
//! 00400020 [e9 16 00 00 00] jmp 1b <detour_function>
//! 00400025 [c3]             ret
//! 2 }
//! 3
//! 4 int detour_function() {
//! 5     return 10;
//! 00400040 [b8 0A 00 00 00] mov eax, 10
//! 00400045 [c3]             ret
//! 6 }
//! ```
//!
//! Beyond what is shown here, a trampoline is also generated so the original
//! function can be called regardless whether the function is hooked or not.
//!
//! *NOTE: Currently x86 & x64 is supported on all major platforms.*

#[macro_use] extern crate cfg_if;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate matches;
extern crate boolinator;
extern crate generic_array;
extern crate mmap_fixed;
extern crate region;
extern crate slice_pool;

// Re-exports
pub use detour::*;
pub use traits::*;

#[macro_use]
mod macros;

// Modules
pub mod error;
mod alloc;
mod arch;
mod detour;
mod pic;
mod traits;
mod util;

#[cfg(test)]
mod tests {
    extern crate volatile_cell;
    use self::volatile_cell::VolatileCell;
    use super::*;

    #[test]
    fn detours_share_target() {
        #[inline(never)]
        extern "C" fn add(x: i32, y: i32) -> i32 {
            VolatileCell::new(x).get() + y
        }

        static_detours! {
            struct Hook1: extern "C" fn (i32, i32) -> i32;
            struct Hook2: extern "C" fn (i32, i32) -> i32;
        }

        let mut hook1 = unsafe { Hook1.initialize(add, |x, y| x - y).unwrap() };

        unsafe { hook1.enable().unwrap() };
        assert_eq!(add(5, 5), 0);

        let mut hook2 = unsafe { Hook2.initialize(add, |x, y| x / y).unwrap() };

        unsafe { hook2.enable().unwrap() };

        // This will call the previous hook's detour
        assert_eq!(hook2.call(5, 5), 0);
        assert_eq!(add(5, 5), 1);
    }

    #[test]
    fn same_detour_and_target() {
        #[inline(never)]
        extern "C" fn add(x: i32, y: i32) -> i32 {
            VolatileCell::new(x).get() + y
        }

        let err = unsafe { RawDetour::new(add as *const (), add as *const()).unwrap_err() };
        assert_matches!(err.kind(), &error::ErrorKind::SameAddress);
    }
}
