# `detour-rs`

This is a cross-platform detour library developed in Rust. Beyond the basic
functionality, this library handles branch redirects, RIP-relative
instructions, and allows the original function to be called using a trampoline
whilst hooked.

This is one of few **cross-platform** detour libraries that exists, and to
maintain this feature, not all desired functionality can be supported due to
lack of cross-platform APIs. Therefore EIP relocation is not supported.

*EIP relocation should be performed whenever a function's prolog instructions
are being executed, simultaneously as the function is being detoured. This is
barely ever an issue, but YMMV.*

## Platforms

- `x86`: Windows, Linux, macOS
- `x64`: Windows, Linux, macOS
- `ARM`: Not implemented, but foundation exists.

## Documentation

https://docs.rs/detour

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
detour = "0.1.0"
```

... and this to your crate root:

```rust
extern crate detour;
```

## Example

### Hello World

```rust
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
```

## TODO

- [ ] Implement macro boilerplate for detouring and calling the original function.