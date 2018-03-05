#![cfg_attr(feature = "nightly", feature(const_fn))]
#[cfg_attr(feature = "nightly", macro_use)]
extern crate detour;

use std::mem;
type FnAdd = extern "C" fn(i32, i32) -> i32;

#[inline(never)]
extern "C" fn sub_detour(x: i32, y: i32) -> i32 {
    unsafe { std::ptr::read_volatile(&x as *const i32) - y }
}

mod raw {
    use detour::RawDetour;
    use super::*;

    #[test]
    fn test() {
        #[inline(never)]
        extern "C" fn add(x: i32, y: i32) -> i32 {
            unsafe { std::ptr::read_volatile(&x as *const i32) + y }
        }

        unsafe {
            let mut hook = RawDetour::new(add as *const (), sub_detour as *const ())
                .expect("target or source is not usable for detouring");

            assert_eq!(add(10, 5), 15);
            assert_eq!(hook.is_enabled(), false);

            hook.enable().unwrap();
            {
                assert!(hook.is_enabled());

                // The `add` function is hooked, but can be called using the trampoline
                let trampoline: FnAdd = mem::transmute(hook.trampoline());

                // Call the original function
                assert_eq!(trampoline(10, 5), 15);

                // Call the hooked function (i.e `add → sub_detour`)
                assert_eq!(add(10, 5), 5);
            }
            hook.disable().unwrap();

            // With the hook disabled, the function is restored
            assert_eq!(hook.is_enabled(), false);
            assert_eq!(add(10, 5), 15);
        }
    }
}

mod generic {
    use detour::GenericDetour;
    use super::*;

    #[test]
    fn test() {
        #[inline(never)]
        extern "C" fn add(x: i32, y: i32) -> i32 {
            unsafe { std::ptr::read_volatile(&x as *const i32) + y }
        }

        unsafe {
            let mut hook = GenericDetour::<FnAdd>::new(add, sub_detour)
                .expect("target or source is not usable for detouring");

            assert_eq!(add(10, 5), 15);
            assert_eq!(hook.call(10, 5), 15);
            hook.enable().unwrap();
            {
                assert_eq!(hook.call(10, 5), 15);
                assert_eq!(add(10, 5), 5);
            }
            hook.disable().unwrap();
            assert_eq!(hook.call(10, 5), 15);
            assert_eq!(add(10, 5), 15);
        }
    }
}

#[cfg(feature = "nightly")]
mod statik {
    use super::*;

    #[inline(never)]
    unsafe extern "C" fn add(x: i32, y: i32) -> i32 {
        std::ptr::read_volatile(&x as *const i32) + y
    }

    static_detours! {
        pub struct DetourAdd: unsafe extern "C" fn(i32, i32) -> i32;
    }

    #[test]
    fn test() {
        unsafe {
            let mut hook = DetourAdd.initialize(add, |x, y| x - y).unwrap();

            assert_eq!(add(10, 5), 15);
            assert_eq!(hook.is_enabled(), false);

            hook.enable().unwrap();
            {
                assert!(hook.is_enabled());
                assert_eq!(hook.call(10, 5), 15);
                assert_eq!(add(10, 5), 5);
            }
            hook.disable().unwrap();

            assert_eq!(hook.is_enabled(), false);
            assert_eq!(hook.call(10, 5), 15);
            assert_eq!(add(10, 5), 15);
        }
    }
}
