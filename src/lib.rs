#![recursion_limit = "1024"]
#![feature(range_contains)]
#![feature(offset_to)]
#![cfg_attr(test, feature(naked_functions))]
#![cfg_attr(test, feature(core_intrinsics))]
#![cfg_attr(test, feature(asm))]

//! A library for cross-platform detours.
//!
//! ## Info
//!
//! This library provides inline detouring functionality by disassembling and
//! patching functions using low-level assembly opcodes, allocated within
//! executable memory. It modifies the target functions in memory and replaces
//! their prolog with an unconditional jump.
//!
//! Beyond the basic functionality this library handles several different edge
//! cases, all of which are mentioned in the *README*.
//!
//! ## Tools
//!
//! Static and dynamic alternatives are available, implemented using
//! [static_detours!](./macro.static_detours.html) or
//! [RawDetour](./struct.RawDetour.html) respectively.  
//! If possible, static detours should be preferred, since they enable
//! type-safety.
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
//! *NOTE: Currently x86/x64 is supported on all major platforms.*

#[macro_use] extern crate cfg_if;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate matches;
extern crate boolinator;
extern crate generic_array;
extern crate mmap;
extern crate region;
extern crate slice_pool;

// Re-exports
pub use variant::RawDetour;
//pub use variant::GenericDetour;

// Modules
pub mod error;
mod alloc;
mod pic;
mod util;
mod variant;

#[macro_use]
mod macros;

#[cfg(feature = "example")]
pub mod example;

cfg_if! {
    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        mod x86;
        use self::x86 as arch;
    } else {
        // Implement ARM support!
    }
}
