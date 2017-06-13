# `detour-rs`

This is a cross-platform detour library developed in Rust. Beyond the basic
functionality, this library handles branch redirects, RIP-relative
instructions, hot-patching, NOP-padded functions, and allows the original
function to be called using a trampoline whilst hooked.

This is one of few **cross-platform** detour libraries that exists, and to
maintain this feature, not all desired functionality can be supported due to
lack of cross-platform APIs. Therefore [EIP relocation](#appendix) is not
supported.

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

## Appendix

- *EIP relocation*

  *Should be performed whenever a function's prolog instructions
  are being executed, simultaneously as the function itself is being
  detoured. This is done by halting all affected threads, copying the related
  instructions and appending a `JMP` to return to the function. This is
  barely ever an issue, and never in single-threaded environments, but YMMV.*

- *NOP-padding*
  ```c
  int function() { return 0; }
  // xor eax, eax
  // ret
  // nop
  // nop
  // ...
  ```
  *Functions such as this one, lacking a hot-patching area, and too small to
  be hooked with a 5-byte `jmp`, are supported thanks to the detection of
  code padding (`NOP/INT3` instructions). Therefore the required amount of
  trailing `NOP` instructions will be replaced, to make room for the detour.*
