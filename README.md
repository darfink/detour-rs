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
- Type-safe detours for any signature (including references), with closures
  as detours.
- Transactions: patch several functions at once, all or nothing, whilst other
  threads are suspended. Threads caught executing the patched instructions
  are moved to equivalent code (EIP relocation).

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

## Thread safety

Enabling a detour overwrites the first instructions of the target. If another
thread is executing those instructions at that moment, it may resume in the
middle of the new jump, and crash. `enable` and `disable` do not guard
against this, which is why they are `unsafe`.

A [`Transaction`][transaction] does. It enables and disables several detours
at once, whilst the chosen threads are suspended. It is applied completely or
not at all: if one change fails, the others are reverted before any thread is
resumed.

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

`Threads::All` suspends every other thread of the process, and
`Threads::Only` a chosen set (e.g. from a `JoinHandle`).

### EIP relocation

A suspended thread may be stopped within the instructions that are about to
be overwritten. Its instruction pointer is then moved to the same instruction
in the trampoline, which holds a copy of the original instructions:

```text
Before                                  After
target:                                 target:
  00400020  push rbp                      00400020  jmp detour
> 00400021  mov rbp, rsp    <- thread     00400025  (rest of the overwritten bytes)
  00400024  sub rsp, 16                   00400028  ...
  00400028  ...
                                        trampoline:
                                          00a00000  push rbp
                                        > 00a00001  mov rbp, rsp    <- thread
                                          00a00004  sub rsp, 16
                                          00a00008  jmp 00400028
```

The thread continues in the trampoline and returns to the target after the
patch. When a detour is disabled, a thread within the trampoline needs no
change, since the trampoline remains valid. A thread that has executed part
of the patch (e.g. the short jump of a hot patch) is moved back to the start
of the target.

If a thread is stopped at an instruction that was rewritten when it was
relocated (e.g. a branch expanded into several instructions), it cannot be
moved. The transaction then fails with `Error::ThreadNotRelocatable`, and
all of its changes are reverted.

| Platform | How threads are suspended |
|----------|---------------------------|
| Windows | `SuspendThread` |
| Apple platforms | `thread_suspend` |
| Linux & Android | A real-time signal (see `linux::set_suspend_signal`) |
| Others | Not supported (only `Threads::None`) |

On AArch64, the patch is usually a single aligned instruction, which is
written atomically. Suspending threads is still recommended there, since a
patch may span several instructions (e.g. an absolute jump).

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

Android is supported, including thread suspension. Other Unix-like systems
(e.g. FreeBSD) are expected to work, but without thread suspension, and are
not tested in CI.

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
- **Other threads.** `enable` and `disable` do not suspend other threads; use
  a `Transaction` (see [Thread safety](#thread-safety)).
- **Blocked signals (Linux & Android).** Threads that block the suspend
  signal cannot be suspended. `Threads::All` skips them (e.g. helper threads
  of the C library), so they must not execute the patched instructions.
- **Dropping detours.** A dropped detour is disabled without suspending
  threads, and its trampoline is released immediately. Disable it with a
  `Transaction` first if other threads may be executing the target.
- **Return addresses.** Only the instruction pointers of suspended threads
  are relocated, not return addresses that refer to patched instructions
  (e.g. of a thread executing a function called from a target's prolog).
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

Instructions are relocated with [`iced-x86`][iced] on x86, which rewrites
relative branches and RIP-relative operands so they still reach the same
addresses. On AArch64, a built-in relocator handles all PC-relative
instructions. The offsets of the relocated instructions are recorded, which
is what allows EIP relocation.

## Upgrading from 0.9

- `GenericDetour` is now `TypedDetour`, and `MemoryError` is now `OsError`.
- `static_detour!` defines a handle type per static, with the same methods
  as before. `StaticDetour` is no longer public; refer to the handle by the
  static's name instead.
- Some error variants were renamed: `InvalidCode` to `InvalidInstruction`,
  `NoPatchArea` to `PatchAreaTooSmall`, and `OutOfMemory` to
  `NoNearbyMemory`.
- `Function` and `HookableWith` are sealed, and `Function` no longer has the
  `Arguments`, `Output` and `Closure` associated types.
- Without `std`, the `no_std` feature is only required on x86.

See the [changelog](./CHANGELOG.md) for all changes, including those of
earlier versions.

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
