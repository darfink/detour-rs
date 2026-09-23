<div align="center">

# `detour-rs`

[![CI status][ci-shield]][ci]
[![crates.io version][crate-shield]][crate]
[![Documentation][docs-shield]][docs]
[![Language (Rust)][rust-shield]][rust]

</div>

This is a cross-platform detour (inline hooking) library developed in Rust.
Beyond the basic functionality, this library handles branch redirects,
RIP/PC-relative instructions, hot-patching, NOP-padded functions, and allows
the original function to be called using a trampoline whilst hooked.

This is one of few **cross-platform** detour libraries that exists, and to
maintain this feature, not all desired functionality can be supported due to
lack of cross-platform APIs. Therefore [EIP relocation](#appendix) is not
supported.

The library works on **stable Rust** (1.85+). It also supports
`#![no_std]` environments with a global allocator; see [Features](#features).

## Platforms

| Architecture | Windows | Linux | macOS | Notes |
|--------------|:-------:|:-----:|:-----:|-------|
| `x86`        | ✓       | ✓     |       | Hot-patching, padding detection |
| `x86-64`     | ✓       | ✓     | ✓     | Relays for detours beyond ±2 GiB |
| `AArch64`    | ✓       | ✓     | ✓     | Relays for detours beyond ±128 MiB, BTI & PAC aware |

Other Unix-like systems (e.g. FreeBSD, Android) are expected to work, but are
not tested in CI. Instruction relocation is powered by [`iced-x86`][iced] on
x86; AArch64 uses a built-in relocator.

WebAssembly is not supported, and cannot be: its code is neither addressable
nor writable at runtime, which inline detouring fundamentally requires.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
detour = "0.9.0"
```

## Example

- A static detour (one of *three* different detours):

```rust
use std::error::Error;
use detour::static_detour;

static_detour! {
  static Test: /* extern "X" */ fn(i32) -> i32;
}

#[inline(never)]
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
```sh
$ cargo build --example messageboxw_detour
```

## Features

- **`std`** (default): Uses the standard library for locking, and allows
  converting `detour::MemoryError` into `std::io::Error`.
- **`no_std`**: Supports `#![no_std]` environments (requires `alloc`), using
  spin locks. An operating system is still required for memory management
  (see the [platforms](#platforms)).

```toml
[dependencies]
detour = { version = "0.9.0", default-features = false, features = ["no_std"] }
```

On x86, `iced-x86` treats `std` and `no_std` as mutually exclusive, so the
`no_std` feature cannot be combined with another crate enabling `iced-x86/std`.

## Upgrading from 0.8

- The `nightly` feature has been removed; all detours, including
  `static_detour!`, work on stable Rust.
- Static detour closures must be `Send + Sync`.
- `RawDetour::trampoline` returns `*const ()` instead of `&()`.
- `Error::RegionFailure` has been replaced by `Error::Memory(MemoryError)`
  (convertible into `std::io::Error`), and `Error` is now `#[non_exhaustive]`.
- Disabling a detour whose target has been modified since (e.g. by another,
  later enabled, detour of the same target) fails with
  `Error::TargetModified` instead of silently overwriting it.
- `extern "cdecl"`, `"stdcall"`, `"fastcall"` & `"thiscall"` function
  pointers are only supported on `x86`; `"win64"` & `"sysv64"` on `x86-64`.

## Mentions

Part of the library's external user interface was inspired by
[minhook-rs][minhook], created by [Jascha-N][minhook-author], and it contains
derivative code of his work.

## Appendix

- *EIP relocation*

  *Should be performed whenever a function's prolog instructions
  are being executed, simultaneously as the function itself is being
  detoured. This is done by halting all affected threads, copying the affected
  instructions and appending a `JMP` to return to the function. This is
  barely ever an issue, and never in single-threaded environments, but YMMV.
  On AArch64, only a single instruction is replaced, which is atomic.*

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
[ci-shield]: https://img.shields.io/github/actions/workflow/status/darfink/detour-rs/ci.yml?branch=master&label=CI&logo=github&style=flat-square
[ci]: https://github.com/darfink/detour-rs/actions/workflows/ci.yml
[crate-shield]: https://img.shields.io/crates/v/detour.svg?style=flat-square
[crate]: https://crates.io/crates/detour
[rust-shield]: https://img.shields.io/badge/powered%20by-rust-blue.svg?style=flat-square
[rust]: https://www.rust-lang.org
[docs-shield]: https://img.shields.io/badge/docs-crates-green.svg?style=flat-square
[docs]: https://docs.rs/detour/
[iced]: https://github.com/icedland/iced
[minhook-author]: https://github.com/Jascha-N
[minhook]: https://github.com/Jascha-N/minhook-rs/
