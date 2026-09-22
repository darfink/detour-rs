//! Shared test utilities.
#![allow(dead_code)]

/// Machine code for a function returning `10`.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
const RET10: &[u8] = &[0xB8, 0x0A, 0x00, 0x00, 0x00, 0xC3]; // mov eax, 10; ret
#[cfg(target_arch = "aarch64")]
const RET10: &[u8] = &[0x40, 0x01, 0x80, 0x52, 0xC0, 0x03, 0x5F, 0xD6]; // mov w0, #10; ret

/// Creates a function returning `10`, located at least `distance` bytes after
/// `near` (i.e. beyond the reach of a relative branch).
pub fn far_ret10(near: usize, distance: usize) -> usize {
  let size = region::page::size();
  let mut hint = (near + distance).next_multiple_of(0x10000);

  for _ in 0..4096 {
    if let Some(address) = map_code(hint, size) {
      if address.abs_diff(near) >= distance {
        return address;
      }
    }
    hint += 0x10_0000;
  }

  panic!("could not map a distant function");
}

#[cfg(unix)]
fn map_code(hint: usize, size: usize) -> Option<usize> {
  #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
  let flags = libc::MAP_JIT;
  #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
  let flags = 0;

  // SAFETY: Anonymous mappings without `MAP_FIXED` never replace memory.
  let address = unsafe {
    libc::mmap(
      hint as *mut libc::c_void,
      size,
      libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
      libc::MAP_PRIVATE | libc::MAP_ANON | flags,
      -1,
      0,
    )
  };

  if address == libc::MAP_FAILED {
    return None;
  }

  // SAFETY: The mapping is writable (for this thread, in the case of JIT).
  unsafe {
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    libc::pthread_jit_write_protect_np(0);
    std::ptr::copy_nonoverlapping(RET10.as_ptr(), address.cast(), RET10.len());
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    libc::pthread_jit_write_protect_np(1);
  }
  clear_cache(address as usize, RET10.len());
  Some(address as usize)
}

#[cfg(windows)]
fn map_code(hint: usize, size: usize) -> Option<usize> {
  let mut memory = region::alloc_at(
    hint as *const u8,
    size,
    region::Protection::READ_WRITE_EXECUTE,
  )
  .ok()?;
  let address = memory.as_mut_ptr::<u8>();
  // SAFETY: The allocation is writable and large enough.
  unsafe { std::ptr::copy_nonoverlapping(RET10.as_ptr(), address, RET10.len()) };
  std::mem::forget(memory);
  Some(address as usize)
}

fn clear_cache(address: usize, len: usize) {
  #[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
  {
    unsafe extern "C" {
      fn sys_icache_invalidate(start: *mut core::ffi::c_void, len: usize);
    }
    // SAFETY: The range is mapped.
    unsafe { sys_icache_invalidate(address as *mut _, len) };
  }

  #[cfg(all(target_arch = "aarch64", not(target_vendor = "apple")))]
  {
    unsafe extern "C" {
      fn __clear_cache(start: *mut core::ffi::c_char, end: *mut core::ffi::c_char);
    }
    // SAFETY: The range is mapped.
    unsafe { __clear_cache(address as *mut _, (address + len) as *mut _) };
  }

  let _ = (address, len);
}
