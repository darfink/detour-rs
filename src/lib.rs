#![recursion_limit = "1024"]
#![feature(range_contains)]
#![cfg_attr(test, feature(naked_functions))]
#![cfg_attr(test, feature(core_intrinsics))]
#![cfg_attr(test, feature(asm))]

#[macro_use] extern crate cfg_if;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate generic_array;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate matches;
extern crate region;
extern crate libc;
extern crate mmap;
extern crate slice_pool;
extern crate boolinator;

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

/// Platform agnostic tests (see `arch` for extensive tests).
#[cfg(test)]
mod tests {
    use std::mem;
    use super::*;

    extern "C" fn add(x: i32, y: i32) -> i32 {
        x + y
    }

    extern "C" fn sub(x: i32, y: i32) -> i32 {
        x - y
    }

    #[test]
    fn inline_basic() {
        unsafe {
            let mut hook = InlineDetour::new(add as *const (), sub as *const ()).unwrap();

            assert_eq!(add(10, 5), 15);
            hook.enable().unwrap();
            {
                let original: extern "C" fn(i32, i32) -> i32 = mem::transmute(hook.callable_address());
                assert_eq!(original(10, 5), 15);
                assert_eq!(add(10, 5), 5);
            }
            hook.disable().unwrap();
            assert_eq!(add(10, 5), 15);
        }
    }
}
