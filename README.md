detour-rs
=========
[![Travis build status][travis-shield]][travis]
[![Appveyor build status][appveyor-shield]][appveyor]
[![crates.io version][crate-shield]][crate]
[![Language (Rust)][rust-shield]][rust]

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
#[macro_use] extern crate detour;
#[macro_use] extern crate lazy_static;

extern "C" fn add(x: i32, y: i32) -> i32 {
    x + y
}

static_detours! {
    struct DetourAdd: extern "C" fn(i32, i32) -> i32;
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
```

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

<!-- Links -->
[travis-shield]: https://img.shields.io/travis/darfink/detour-rs.svg?style=flat-square
[travis]: https://travis-ci.org/darfink/detour-rs
[appveyor-shield]: https://img.shields.io/appveyor/ci/darfink/detour-rs/master.svg?style=flat-square
[appveyor]: https://ci.appveyor.com/project/darfink/detour-rs
[crate-shield]: https://img.shields.io/crates/v/detour.svg?style=flat-square
[crate]: https://crates.io/crates/detour
[rust-shield]: https://img.shields.io/badge/powered%20by-rust-blue.svg?style=flat-square
[rust]: https://www.rust-lang.org