#[macro_use] extern crate lazy_static;
#[macro_use] extern crate detour;
extern crate volatile_cell;

use std::mem;
use volatile_cell::VolatileCell;

type FnAdd = extern "C" fn(i32, i32) -> i32;

#[inline(never)]
extern "C" fn add(x: i32, y: i32) -> i32 {
    VolatileCell::new(x).get() + y
}

#[inline(never)]
extern "C" fn sub_detour(x: i32, y: i32) -> i32 {
    VolatileCell::new(x).get() - y
}

#[test]
fn basics() {
    unsafe {
        let mut hook = detour::RawDetour::new(add as *const (), sub_detour as *const ())
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

static_detours! {
    pub struct DetourAdd: extern "C" fn(i32, i32) -> i32;
}

#[test]
fn static_hook() {
    unsafe {
        let mut hook = DetourAdd::initialize(add, |x, y| x - y).unwrap();

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
