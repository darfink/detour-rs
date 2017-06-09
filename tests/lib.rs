extern crate detour;
use std::mem;

type FnAdd = extern "C" fn(i32, i32) -> i32;

extern "C" fn add(x: i32, y: i32) -> i32 {
    x + y
}

extern "C" fn sub_detour(x: i32, y: i32) -> i32 {
    x - y
}

#[test]
fn basics() {
    unsafe {
        let mut hook = detour::InlineDetour::new(add as *const (), sub_detour as *const ())
          .expect("target or source is not usable for detouring");

        assert_eq!(add(10, 5), 15);
        hook.enable().unwrap();
        {
          // The `add` function is hooked, but can be called using the trampoline
          let trampoline: FnAdd = mem::transmute(hook.callable_address());

          // Call the original function
          assert_eq!(trampoline(10, 5), 15);

          // Call the hooked function (i.e `add → sub_detour`)
          assert_eq!(add(10, 5), 5);
        }
        hook.disable().unwrap();

        // With the hook disabled, the function is restored
        assert_eq!(add(10, 5), 15);
    }
}