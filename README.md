<div align="center">

# `detour-rs`

## Cross-platform function detouring

[![GitHub CI Status][github-shield]][github]
[![crates.io version][crate-shield]][crate]
[![Documentation][docs-shield]][docs]
[![License][license-shield]][license]

</div>

This is a cross-platform detour (inline hooking) library developed in Rust.
Beyond the basic functionality, this library handles branch redirects,
RIP/PC-relative instructions, hot-patching, NOP-padded functions, and allows
the original function to be called using a trampoline whilst hooked.

Patches are kept as small as possible: on AArch64 a single aligned
instruction is replaced atomically, and x86 hot-patching only alters the
2-byte instruction at the function's entry (the jump itself is placed in the
padding preceding it). Other threads are not suspended while a detour is
toggled, and their instruction pointers are not relocated (i.e. no
[EIP relocation](#appendix), yet). In practice this only matters when another
thread executes the target's first few instructions at the exact moment it is
patched.

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

- Hooking a program from its very start, so no invocation is missed, is
  shown in [`early_hook`](./examples/early_hook.rs). The library installs its
  detours when loaded, before the program's `main` runs:
```sh
$ cargo build --example early_hook --example early_target --example launch_suspended

# Linux
$ LD_PRELOAD=target/debug/examples/libearly_hook.so target/debug/examples/early_target

# macOS
$ DYLD_INSERT_LIBRARIES=target/debug/examples/libearly_hook.dylib target/debug/examples/early_target

# Windows (starts the program suspended, and loads the library before its entry point)
$ target\debug\examples\launch_suspended.exe target\debug\examples\early_hook.dll target\debug\examples\early_target.exe
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

  *If another thread is executing a target's first instructions while they
  are replaced, it may resume in the middle of the new jump. Some libraries
  prevent this by suspending all other threads, and moving any instruction
  pointer within the patched bytes to the equivalent position in the
  trampoline. This library does not do so yet. The risk is mostly limited to
  x86, where a 5-byte jump may replace several instructions; on AArch64 a
  single instruction is replaced atomically (except for the absolute-jump
  fallback, used when no memory is available within ±128 MiB of the target).
  Enable detours before other threads run the target (e.g. at start-up; see
  the `early_hook` example) to avoid the issue entirely.*

- *Rosetta 2*

  *Under Rosetta 2 (x86-64 code on Apple silicon), modifying code whilst
  another thread executes the same memory page may intermittently raise
  `SIGBUS`. This is a limitation of the translator; native Intel Macs and
  native Apple silicon code are not affected.*

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
[github-shield]: https://img.shields.io/github/actions/workflow/status/darfink/detour-rs/ci.yml?branch=master&label=actions&logo=github&style=for-the-badge
[github]: https://github.com/darfink/detour-rs/actions/workflows/ci.yml?query=branch%3Amaster
[crate-shield]: https://img.shields.io/crates/v/detour.svg?style=for-the-badge
[crate]: https://crates.io/crates/detour
[docs-shield]: https://img.shields.io/badge/docs-crates-green.svg?style=for-the-badge
[docs]: https://docs.rs/detour/
[license-shield]: https://img.shields.io/crates/l/detour.svg?style=for-the-badge
[license]: https://github.com/darfink/detour-rs
[iced]: https://github.com/icedland/iced
[minhook-author]: https://github.com/Jascha-N
[minhook]: https://github.com/Jascha-N/minhook-rs/
