//! Memory primitives: querying, patching existing code, and allocating new
//! executable memory close to a given address.

pub(crate) use self::alloc::{CodeBlock, allocate_near};

use crate::error::{MemoryError, Result};
use crate::sync::Mutex;

mod alloc;
#[cfg(target_vendor = "apple")]
mod apple;

/// Serializes all code modifications performed by this crate.
///
/// Creating, enabling, and disabling detours all read or write code that other
/// detours may also touch (e.g. two detours sharing a target, or relocated
/// code residing in a shared page).
static PATCH_LOCK: Mutex<()> = Mutex::new(());

/// Acquires the global patch lock, which is held until the guard is dropped.
#[must_use]
pub(crate) fn patch_lock() -> impl Sized {
  PATCH_LOCK.lock()
}

/// Returns whether `address` resides in executable memory.
pub(crate) fn is_executable(address: *const ()) -> Result<bool> {
  match region::query(address) {
    Ok(region) => Ok(region.is_executable()),
    Err(region::Error::UnmappedRegion) => Ok(false),
    Err(error) => Err(MemoryError::from_region(error).into()),
  }
}

/// Returns the number of contiguous, readable bytes at `address` (at most
/// `max`).
pub(crate) fn readable_len(address: usize, max: usize) -> usize {
  let Ok(regions) = region::query_range(address as *const u8, max) else {
    return 0;
  };

  let end = address.saturating_add(max);
  let mut cursor = address;

  for region in regions {
    let Ok(region) = region else { break };
    let range = region.as_range();

    if range.start > cursor || !region.is_readable() {
      break;
    }

    cursor = range.end;
    if cursor >= end {
      break;
    }
  }

  cursor.min(end).saturating_sub(address)
}

/// Copies `bytes` to `destination` without calling into `memcpy`.
///
/// Code modifications may happen while memory is temporarily non-executable,
/// or while a hooked function is being redirected. Using a plain volatile
/// loop ensures no (possibly hooked) library routine is invoked meanwhile.
///
/// # Safety
///
/// `destination` must be valid for writes of `bytes.len()` bytes.
pub(crate) unsafe fn copy_code(destination: *mut u8, bytes: &[u8]) {
  // A single, aligned store cannot be observed partially by other threads
  // (e.g. a 4-byte AArch64 branch, or a 5-byte x86 jump within a word).
  const WORD: usize = size_of::<usize>();
  let offset = destination as usize % WORD;
  if !bytes.is_empty() && offset + bytes.len() <= WORD {
    let word = destination.wrapping_sub(offset).cast::<usize>();
    // SAFETY: The aligned word containing the destination range is part of
    // the same page, which the caller guarantees to be readable & writable.
    unsafe {
      let mut value = word.read_volatile().to_ne_bytes();
      value[offset..offset + bytes.len()].copy_from_slice(bytes);
      word.write_volatile(usize::from_ne_bytes(value));
    }
    return;
  }

  for (index, byte) in bytes.iter().enumerate() {
    // SAFETY: The caller guarantees that the destination range is writable.
    unsafe { destination.add(index).write_volatile(*byte) };
  }
}

/// Overwrites existing (typically read-only) code at `address` with `bytes`.
///
/// The memory protection is restored afterwards, and the instruction cache is
/// invalidated for the modified range.
///
/// # Safety
///
/// The caller must guarantee that the modification leaves the process in a
/// consistent state, i.e. that `address` points to code that may be replaced.
pub(crate) unsafe fn patch_code(address: *mut u8, bytes: &[u8]) -> Result<()> {
  // SAFETY: The requested protection is a superset of the existing one.
  let result = match unsafe {
    region::protect_with_handle(address, bytes.len(), region::Protection::READ_WRITE_EXECUTE)
  } {
    Ok(_guard) => {
      // SAFETY: The range was just made writable.
      unsafe { copy_code(address, bytes) };
      Ok(())
    },
    // SAFETY: Forwarded from the caller.
    #[cfg(target_vendor = "apple")]
    Err(_) => unsafe { apple::patch_code(address, bytes) },
    #[cfg(not(target_vendor = "apple"))]
    Err(error) => Err(MemoryError::from_region(error).into()),
  };

  if result.is_ok() {
    flush_instruction_cache(address, bytes.len());
  }
  result
}

/// Ensures that modified code is visible to instruction fetches.
pub(crate) fn flush_instruction_cache(address: *const u8, len: usize) {
  #[cfg(windows)]
  {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: Flushing an arbitrary range of the current process is benign.
    unsafe { FlushInstructionCache(GetCurrentProcess(), address.cast(), len) };
  }

  #[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
  {
    unsafe extern "C" {
      fn sys_icache_invalidate(start: *mut core::ffi::c_void, len: usize);
    }

    // SAFETY: Invalidating an arbitrary mapped range is benign.
    unsafe { sys_icache_invalidate(address.cast_mut().cast(), len) };
  }

  #[cfg(all(target_arch = "aarch64", not(target_vendor = "apple"), not(windows)))]
  // SAFETY: Cache maintenance by virtual address is permitted at EL0 on all
  // supported operating systems (this mirrors compiler-rt's
  // `__clear_cache`).
  unsafe {
    aarch64_clear_cache(address as usize, len)
  };

  // x86 and x86-64 keep instruction caches coherent with data writes.
  let _ = (address, len);
}

/// Cleans the data cache and invalidates the instruction cache for a range.
#[cfg(all(target_arch = "aarch64", not(target_vendor = "apple"), not(windows)))]
unsafe fn aarch64_clear_cache(start: usize, len: usize) {
  use core::arch::asm;

  let end = start + len;
  let ctr: u64;
  // SAFETY: CTR_EL0 is readable from user space.
  unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };

  // IDC: data cache clean to the point of unification is not required.
  if ctr & (1 << 28) == 0 {
    let line = 4usize << ((ctr >> 16) & 0xF);
    let mut address = start & !(line - 1);
    while address < end {
      // SAFETY: Cleaning a cache line has no architectural side effects.
      unsafe { asm!("dc cvau, {}", in(reg) address, options(nostack)) };
      address += line;
    }
  }
  // SAFETY: Barrier.
  unsafe { asm!("dsb ish", options(nostack)) };

  // DIC: instruction cache invalidation is not required.
  if ctr & (1 << 29) == 0 {
    let line = 4usize << (ctr & 0xF);
    let mut address = start & !(line - 1);
    while address < end {
      // SAFETY: Invalidating a cache line has no architectural side effects.
      unsafe { asm!("ic ivau, {}", in(reg) address, options(nostack)) };
      address += line;
    }
    // SAFETY: Barrier.
    unsafe { asm!("dsb ish", options(nostack)) };
  }
  // SAFETY: Barrier.
  unsafe { asm!("isb", options(nostack)) };
}
