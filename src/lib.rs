#![recursion_limit = "1024"]
#![feature(naked_functions)]
#![feature(range_contains)]
#![feature(core_intrinsics)]
#![feature(asm)]

#[macro_use]
extern crate generic_array;

#[macro_use]
extern crate error_chain;
extern crate region;
extern crate libc;
extern crate memmap;

// Re-exports
pub use vmt::Virtual;
pub use inline::Inline;

// Modules
pub mod error;
mod inline;
mod util;
mod vmt;

/// Interface for a detour trait object.
pub trait Detour: Drop {
    /// Enable the detour.
    unsafe fn enable(&mut self) -> error::Result<()>;

    /// Disable the detour.
    unsafe fn disable(&mut self) -> error::Result<()>;

    /// Returns a callable address to the original function.
    fn callable_address(&self) -> *const ();

    /// Returns whether the target is hooked or not.
    fn is_hooked(&self) -> bool;
}

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

    // Represents a C++ object with a virtual method table
    struct VirtualObject {
        pub vtable: *const [*const (); 1],
        pub value: i32,
    }

    #[test]
    fn vmt_basic() {
        let table = [add as *const ()];
        let vo = VirtualObject { vtable: &table, value: 5 };

        unsafe {
            let replacement = mem::transmute(&vmt_basic);
            let mut hook = Virtual::new(&vo, 0, replacement).unwrap();

            assert_eq!(hook.is_hooked(), false);
            assert!((*vo.vtable)[0] == add as *const ());

            hook.enable().unwrap();
            {
                assert!((*vo.vtable)[0] == replacement);
                assert!(hook.is_hooked());
            }
            hook.disable().unwrap();

            assert_eq!(hook.is_hooked(), false);
            assert!((*vo.vtable)[0] == add as *const ());
        }
    }

    #[test]
    fn inline_basic() {
        unsafe {
            let mut hook = Inline::new(add as *const (), sub as *const ()).unwrap();

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

    //static_hook! {
    //    DetourAdd: extern "C" fn(i32, i32) -> i32;
    //}

    //#[test]
    //fn it_works() {
    //    let mut detour = DetourAdd::new(&add, |x, y| x - y);

    //    assert_eq!(add(5, 5), 10);
    //    detour.toggle(|| assert_eq!(add(5, 5), 0));
    //    assert_eq!(add(5, 5), 10);
    //}
}
