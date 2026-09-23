//! A `#![no_std]` executable using detours, linked against the C library only
//! (for allocation and output).
#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use detour::{GenericDetour, RawDetour, static_detour};

/// Allocates memory using the C library.
struct Malloc;

// SAFETY: `posix_memalign` & `free` satisfy the `GlobalAlloc` contract.
unsafe impl core::alloc::GlobalAlloc for Malloc {
  unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
    let mut pointer = core::ptr::null_mut();
    let align = layout.align().max(size_of::<usize>());
    // SAFETY: The alignment is a power of two, and a multiple of a pointer.
    match unsafe { libc::posix_memalign(&mut pointer, align, layout.size()) } {
      0 => pointer.cast(),
      _ => core::ptr::null_mut(),
    }
  }

  unsafe fn dealloc(&self, pointer: *mut u8, _: core::alloc::Layout) {
    // SAFETY: The pointer was allocated using `posix_memalign`.
    unsafe { libc::free(pointer.cast()) };
  }
}

#[global_allocator]
static ALLOCATOR: Malloc = Malloc;

/// Required by the precompiled `core` & `alloc` crates, despite `panic = "abort"`
/// (unwinding never occurs).
#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() {}

/// See `rust_eh_personality`.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "C" fn _Unwind_Resume() -> ! {
  // SAFETY: Terminates the process.
  unsafe { libc::abort() }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
  print(c"panic\n");
  // SAFETY: Terminates the process.
  unsafe { libc::abort() }
}

fn print(message: &core::ffi::CStr) {
  // SAFETY: The string is null-terminated.
  unsafe { libc::printf(c"%s".as_ptr(), message.as_ptr()) };
}

#[inline(never)]
extern "C" fn add_one(value: u32) -> u32 {
  core::hint::black_box(value) + 1
}

#[inline(never)]
extern "C" fn add_two(value: u32) -> u32 {
  core::hint::black_box(value) + 2
}

#[inline(never)]
extern "C" fn add_three(value: u32) -> u32 {
  core::hint::black_box(value) + 3
}

extern "C" fn times_hundred(value: u32) -> u32 {
  value * 100
}

static_detour! {
  static AddThree: extern "C" fn(u32) -> u32;
}

fn run() -> detour::Result<()> {
  // SAFETY: The functions share the same signature, and no other threads exist.
  unsafe {
    let raw = RawDetour::new(add_one as *const (), times_hundred as *const ())?;
    raw.enable()?;
    let original: extern "C" fn(u32) -> u32 = core::mem::transmute(raw.trampoline());
    assert_eq!(add_one(5), 500);
    assert_eq!(original(5), 6);
    raw.disable()?;
    assert_eq!(add_one(5), 6);

    let generic = GenericDetour::<extern "C" fn(u32) -> u32>::new(add_two, times_hundred)?;
    generic.enable()?;
    assert_eq!(add_two(5), 500);
    assert_eq!(generic.call(5), 7);
    drop(generic);
    assert_eq!(add_two(5), 7);

    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    AddThree
      .initialize(add_three, move |value| {
        counter.fetch_add(1, Ordering::SeqCst);
        AddThree.call(value) * 2
      })?
      .enable()?;
    assert_eq!(add_three(5), 16);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    AddThree.disable()?;
    assert_eq!(add_three(5), 8);
  }
  Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn main(_argc: libc::c_int, _argv: *const *const libc::c_char) -> libc::c_int {
  match run() {
    Ok(()) => {
      print(c"no_std detours: ok\n");
      0
    },
    Err(_) => {
      print(c"no_std detours: error\n");
      1
    },
  }
}
