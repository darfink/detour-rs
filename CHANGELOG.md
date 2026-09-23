# Changelog

## 0.9.0 (unreleased)

A comprehensive overhaul of the library.

### Added

- `#![no_std]` support (with `alloc`), using the `no_std` feature.
- AArch64 support (Linux, macOS & Windows), including relays for detours
  beyond ±128 MiB, relocation of all PC-relative instructions, and awareness
  of BTI & PAC landing pads.
- Support for Apple silicon, where code pages are patched by remapping (code
  signing prevents making `__TEXT` writable), and trampolines are allocated
  using `MAP_JIT`.
- Support for x86-64 macOS without linker workarounds (`-segprot`).
- Support for systems enforcing W^X (executable memory is re-protected when
  written to, if read-write-execute mappings are refused).
- `static_detour!` supports trailing commas in argument lists (#30).
- `extern "C-unwind"`, `"system-unwind"`, `"sysv64"` function pointers.
- `Error::TargetModified`, returned instead of overwriting code modified by a
  third party (e.g. another detour of the same target).
- GitHub Actions CI for Linux, Windows & macOS on x86, x86-64 & AArch64.

### Changed

- **Works on stable Rust** (1.85+); the `nightly` feature has been removed
  (#35, #39, #47).
- Instruction decoding & relocation uses `iced-x86` instead of the
  unmaintained, C-based `libudis86-sys` (#42). As a result, `loop`/`jrcxz`
  instructions and branches within the prolog are now relocated.
- Executable memory is allocated using `mmap` hints (Unix) & `VirtualAlloc`
  (Windows) instead of `mmap-fixed` (#32) and `slice-pool`, never replacing
  existing mappings.
- Static detour closures must be `Send + Sync`.
- The `Function` trait has a new associated type, `Closure`.
- `RawDetour::trampoline` returns `*const ()`.
- `Error::RegionFailure` was replaced by `Error::Memory(MemoryError)`, which is
  convertible into `std::io::Error`, and `Error` is `#[non_exhaustive]`.
- Calling-convention specific function pointers are restricted to the
  architectures that support them.
- The crate uses the 2024 edition.

### Fixed

- Detours were never disabled when dropped in release builds.
- Replacing a static detour's closure could free it whilst in use by another
  thread.
- Trampolines & relays were freed whilst still referenced by a target that
  failed to be restored.
- The instruction cache is invalidated after code is modified.
- Relocated instructions reading the patched bytes (RIP-relative) are rejected
  instead of silently reading the detour jump.
- Position-independent i686 code (`call next; pop reg`) observed the
  trampoline's address.

### Removed

- Dependencies: `cfg-if`, `generic-array`, `lazy_static`, `libudis86-sys`,
  `mmap-fixed`, `slice-pool`, `matches` & `winapi`.
