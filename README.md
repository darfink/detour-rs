detour-rs
=========
[![Travis build status][travis-shield]][travis]
[![Appveyor build status][appveyor-shield]][appveyor]
[![crates.io version][crate-shield]][crate]
[![Documentation][docs-shield]][docs]
[![Language (Rust)][rust-shield]][rust]

This is a cross-platform detour library developed in Rust. Beyond the basic
functionality, this library handles branch redirects, RIP-relative
instructions, hot-patching, NOP-padded functions, and allows the original
function to be called using a trampoline whilst hooked.

This is one of few **cross-platform** detour libraries that exists, and to
maintain this feature, not all desired functionality can be supported due to
lack of cross-platform APIs. Therefore [EIP relocation](#appendix) is not
supported.

**NOTE**: Nightly is currently required for `static_detour!` and is enabled by
default.

## Platforms

- `x86/x64`: Windows, Linux & macOS.
- `ARM`: Not implemented, but foundation exists.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
detour = "0.7.0"
```

## Example

- A static detour (one of *three* different detours):

```rust
use std::error::Error;
use detour::static_detour;

static_detour! {
  static Test: /* extern "X" */ fn(i32) -> i32;
}

fn add5(val: i32) -> i32 {
  val + 5
}

fn add10(val: i32) -> i32 {
  val + 10
}

fn main() -> Result<(), Box<dyn Error>> {
  // Reroute the 'add5' function to 'add10' (can also be a closure)
  unsafe { Test.initialize(add5, add10)? };

  assert_eq!(add5(1), 6);
  assert_eq!(Test.call(1), 6);

  // Hooks must be enabled to take effect
  unsafe { Test.enable()? };

  // The original function is detoured to 'add10'
  assert_eq!(add5(1), 11);

  // The original function can still be invoked using 'call'
  assert_eq!(Test.call(1), 6);

  // It is also possible to change the detour whilst hooked
  Test.set_detour(|val| val - 5);
  assert_eq!(add5(5), 0);

  unsafe { Test.disable()? };

  assert_eq!(add5(1), 6);
  Ok(())
}
```

- A Windows API hooking example is available [here](./examples/messageboxw_detour.rs); build it by running:
```
$ cargo build --example messageboxw_detour
```

## Mentions

Part of the library's external user interface was inspired by
[minhook-rs][minhook], created by [Jascha-N][minhook], and it contains
derivative code of his work.

## Appendix

- *EIP relocation*

  *Should be performed whenever a function's prolog instructions
  are being executed, simultaneously as the function itself is being
  detoured. This is done by halting all affected threads, copying the affected
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
[travis-shield]: https://img.shields.io/travis/darfink/detour-rs.svg?style=flat-square&label=travis
[travis]: https://travis-ci.org/darfink/detour-rs
[appveyor-shield]: https://img.shields.io/appveyor/ci/darfink/detour-rs/master.svg?style=flat-square&label=appveyor
[appveyor]: https://ci.appveyor.com/project/darfink/detour-rs
[crate-shield]: https://img.shields.io/crates/v/detour.svg?style=flat-square
[crate]: https://crates.io/crates/detour
[rust-shield]: https://img.shields.io/badge/powered%20by-rust-blue.svg?style=flat-square
[rust]: https://www.rust-lang.org
[docs-shield]: https://img.shields.io/badge/docs-crates-green.svg?style=flat-square
[docs]: https://docs.rs/detour/
[minhook-author]: https://github.com/Jascha-N
[minhook]: https://github.com/Jascha-N/minhook-rs/
