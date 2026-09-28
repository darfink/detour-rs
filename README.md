<div align="center">

# `detour-rs`

## Cross-platform function detouring

[![GitHub CI Status][github-shield]][github]
[![crates.io version][crate-shield]][crate]
[![Documentation][docs-shield]][docs]

</div>

A cross-platform library for detouring (inline hooking) functions at runtime.
It redirects a function to your own code, while the original remains callable
through a trampoline.

- Works on **stable Rust** (1.85+), with optional `#![no_std]` support.
- Supports `x86`, `x86-64` & `AArch64` on Windows, Linux & macOS (including
  Apple silicon).
- Relocates branches and RIP/PC-relative instructions, supports hot-patching
  and NOP-padded functions, and reaches distant detours through relays.
- Type-safe detours for any function pointer, including closures as detours.

## Installation

```toml
[dependencies]
detour = "0.9.0"
```

## Quick start

```rust
use detour::static_detour;
use std::error::Error;

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

## Choosing a detour

| Type | Detour | Type safety | Defined |
|------|--------|-------------|---------|
| [`static_detour!`][static] | Function or closure | Enforced | Statically |
| [`TypedDetour`][typed] | Function | Enforced | At runtime |
| [`RawDetour`][raw] | Function | None (raw pointers) | At runtime |

`TypedDetour` requires no macro, and is suitable when the detour is a plain
function:

```rust
use detour::TypedDetour;

#[inline(never)]
extern "C" fn multiply(a: i32, b: i32) -> i32 {
  std::hint::black_box(a) * b
}

extern "C" fn add(a: i32, b: i32) -> i32 {
  a + b
}

fn main() -> detour::Result<()> {
  let hook = unsafe { TypedDetour::<extern "C" fn(i32, i32) -> i32>::new(multiply, add)? };
  unsafe { hook.enable()? };

  assert_eq!(multiply(2, 3), 5);
  assert_eq!(hook.call(2, 3), 6);
  Ok(())
}
```

Signatures with references, such as `fn(&str) -> usize`, are supported by
`static_detour!` as is. For `TypedDetour`, name the signature with
[`signature!`][signature] first:

```rust
use detour::{TypedDetour, signature};

signature! {
  struct Length(fn(&str) -> usize);
}

#[inline(never)]
fn length(text: &str) -> usize {
  std::hint::black_box(text).len()
}

fn zero(_: &str) -> usize {
  0
}

fn main() -> detour::Result<()> {
  let hook = unsafe { TypedDetour::new(Length(length), Length(zero))? };
  unsafe { hook.enable()? };

  assert_eq!(length("detour"), 0);
  assert_eq!(unsafe { hook.trampoline() }.call("detour"), 6);
  Ok(())
}
```

`RawDetour` accepts any pointer, e.g. for functions whose signature is only
known at runtime.

Several detours can be enabled or disabled at once with a
[`Transaction`][transaction]. It is applied completely or not at all, and can
suspend other threads whilst the code is patched:

```rust
use detour::{Threads, Transaction, TypedDetour};

#[inline(never)]
fn add(x: i32, y: i32) -> i32 {
  x + y
}

#[inline(never)]
fn sub(x: i32, y: i32) -> i32 {
  x - y
}

fn zero(_: i32, _: i32) -> i32 {
  0
}

fn main() -> detour::Result<()> {
  let add_hook = unsafe { TypedDetour::<fn(i32, i32) -> i32>::new(add, zero)? };
  let sub_hook = unsafe { TypedDetour::<fn(i32, i32) -> i32>::new(sub, zero)? };

  let mut transaction = Transaction::new();
  transaction.enable(&add_hook).enable(&sub_hook);
  unsafe { transaction.commit(Threads::All)? };

  assert_eq!((add(2, 3), sub(2, 3)), (0, 0));
  Ok(())
}
```

## Examples

- [`messageboxw_detour`](./examples/messageboxw_detour.rs): a DLL that
  detours `MessageBoxW` for the process it is injected into (Windows).
- [`early_hook`](./examples/early_hook.rs): installs a detour as soon as the
  library is loaded, before the program's `main` runs, so no invocation is
  missed. [`launch_suspended`](./examples/launch_suspended.rs) loads it on
  Windows.

```sh
$ cargo build --example early_hook --example early_target --example launch_suspended

# Linux
$ LD_PRELOAD=target/debug/examples/libearly_hook.so target/debug/examples/early_target

# macOS (not for SIP-protected or hardened-runtime binaries)
$ DYLD_INSERT_LIBRARIES=target/debug/examples/libearly_hook.dylib target/debug/examples/early_target

# Windows
$ target\debug\examples\launch_suspended.exe target\debug\examples\early_hook.dll target\debug\examples\early_target.exe
```

## Platforms

| Architecture | Windows | Linux | macOS | Notes |
|--------------|:-------:|:-----:|:-----:|-------|
| `x86`        | ✓       | ✓     |       | Hot-patching, padding detection |
| `x86-64`     | ✓       | ✓     | ✓     | Relays for detours beyond ±2 GiB |
| `AArch64`    | ✓       | ✓     | ✓     | Relays for detours beyond ±128 MiB, BTI & PAC aware |

Other Unix-like systems (e.g. FreeBSD, Android) are expected to work, but are
not tested in CI. Instruction relocation uses [`iced-x86`][iced] on x86, and a
built-in relocator on AArch64.

WebAssembly is not supported, and cannot be: its code is neither addressable
nor writable at runtime, which inline detouring fundamentally requires.

## Cargo features

- **`std`** (default): Uses the standard library for locking, and allows
  converting `detour::OsError` into `std::io::Error`, and `JoinHandle` into
  `detour::Thread`.

Disabling `std` supports `#![no_std]` environments (requires `alloc`), using
spin locks. An operating system is still required for memory management. On
x86, enable the `no_std` feature instead (it has no effect on AArch64):

```toml
[dependencies]
detour = { version = "0.9.0", default-features = false, features = ["no_std"] }
```

`iced-x86` requires exactly one of its `std` and `no_std` features, so on x86
`no_std` cannot be combined with another crate enabling `iced-x86/std`.

## Caveats

- **Inlined calls cannot be detoured.** Only calls that actually jump to the
  target are redirected; mark your own targets `#[inline(never)]`. To detour
  a function of another module (e.g. a system library), resolve its address
  at runtime (`dlsym`, `GetProcAddress`), since a direct reference may resolve
  to a local import stub instead.
- **Other threads.** `enable` and `disable` do not suspend other threads. A
  thread executing a target's first instructions at the exact moment they are
  replaced may resume in the middle of the new jump. To avoid this, commit a
  [`Transaction`][transaction] with `Threads::All` (or selected threads): the
  threads are suspended, and any executing the patched instructions are moved
  to equivalent code (EIP relocation). This is supported on Windows, Apple
  platforms, Linux and Android (using a real-time signal). On AArch64 a
  single aligned instruction is replaced atomically, which avoids the issue
  in most cases.
- **Dropping detours.** A dropped detour is disabled without suspending
  threads, and its trampoline is released immediately. Disable it with a
  `Transaction` first if other threads may be executing the target.
- **Return addresses.** Only the program counters of suspended threads are
  relocated, not return addresses that refer to patched instructions.
- **Shared targets.** Multiple detours of the same target must be disabled in
  the reverse order they were enabled in; otherwise
  `Error::TargetModified` is returned.
- **Rosetta 2.** Under Rosetta 2 (x86-64 code on Apple silicon), modifying
  code whilst another thread executes the same memory page may intermittently
  raise `SIGBUS`. Native Intel Macs and native Apple silicon code are not
  affected.

## How it works

To illustrate a detour on x86:

```c
int return_five() {
    return 5;
00400020 [b8 05 00 00 00] mov eax, 5
00400025 [c3]             ret
}

int detour_function() {
    return 10;
00400040 [b8 0a 00 00 00] mov eax, 10
00400045 [c3]             ret
}
```

The target's prolog is disassembled, relocated to a trampoline allocated near
the target, and followed by a jump back to the remainder of the function. The
prolog is then replaced with a jump to the detour:

```c
int return_five() {
    return detour_function();
00400020 [e9 1b 00 00 00] jmp 00400040 <detour_function>
00400025 [c3]             ret
}
```

If the detour is out of reach of a relative jump, the jump targets a relay (an
absolute jump allocated near the target) instead. Functions too small for a
5-byte jump are supported if they are followed by padding (`nop`/`int3`), or
preceded by a hot-patching area, in which case a 2-byte jump at the entry
leads to the 5-byte jump placed in the area. On AArch64, a single `B`
instruction is replaced instead.

## Upgrading from 0.8

- The `nightly` feature has been removed; all detours, including
  `static_detour!`, work on stable Rust.
- Static detour closures must be `Send + Sync`.
- `RawDetour::trampoline` returns `*const ()` instead of `&()`.
- `Error::RegionFailure` has been replaced by `Error::Memory(OsError)`
  (convertible into `std::io::Error`), and `Error` is now `#[non_exhaustive]`.
- Disabling a detour whose target has been modified since (e.g. by another,
  later enabled, detour of the same target) fails with
  `Error::TargetModified` instead of silently overwriting it.
- `extern "cdecl"`, `"stdcall"`, `"fastcall"` & `"thiscall"` function
  pointers are only supported on `x86`; `"win64"` & `"sysv64"` on `x86-64`.

See the [changelog](./CHANGELOG.md) for all changes.

## Acknowledgements

Part of the library's external user interface was inspired by
[minhook-rs][minhook], created by [Jascha-N][minhook-author], and it contains
derivative code of his work.

<!-- Links -->
[github-shield]: https://img.shields.io/github/actions/workflow/status/darfink/detour-rs/ci.yml?branch=master&label=actions&logo=github&style=for-the-badge
[github]: https://github.com/darfink/detour-rs/actions/workflows/ci.yml?query=branch%3Amaster
[crate-shield]: https://img.shields.io/crates/v/detour.svg?style=for-the-badge
[crate]: https://crates.io/crates/detour
[docs-shield]: https://img.shields.io/badge/docs-crates-green.svg?style=for-the-badge
[docs]: https://docs.rs/detour/
[static]: https://docs.rs/detour/latest/detour/macro.static_detour.html
[signature]: https://docs.rs/detour/latest/detour/macro.signature.html
[typed]: https://docs.rs/detour/latest/detour/struct.TypedDetour.html
[raw]: https://docs.rs/detour/latest/detour/struct.RawDetour.html
[transaction]: https://docs.rs/detour/latest/detour/struct.Transaction.html
[iced]: https://github.com/icedland/iced
[minhook-author]: https://github.com/Jascha-N
[minhook]: https://github.com/Jascha-N/minhook-rs/
