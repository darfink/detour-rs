#![recursion_limit = "1024"]
#![feature(range_contains)]
#![cfg_attr(test, feature(naked_functions))]
#![cfg_attr(test, feature(core_intrinsics))]
#![cfg_attr(test, feature(asm))]

#[macro_use] extern crate cfg_if;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate matches;
extern crate boolinator;
extern crate generic_array;
extern crate libc;
extern crate mmap;
extern crate region;
extern crate slice_pool;

// Re-exports
pub use self::detour::InlineDetour;

// Modules
pub mod error;
mod alloc;
mod detour;
mod pic;
mod util;

#[macro_use]
mod macros;

cfg_if! {
    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        mod x86;
        use self::x86 as arch;
    } else {
        // Implement ARM support!
    }
}