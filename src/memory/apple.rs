//! Code patching strategies specific to Apple platforms.
//!
//! The text segment of Mach-O images has a maximum protection of `r-x`, and
//! on Apple silicon, executable pages must additionally be code signed.
//! Therefore `mprotect(PROT_WRITE)` is refused and two alternatives are used:
//!
//! 1. `vm_protect(VM_PROT_COPY)`, which creates a private, writable copy of
//!    the pages (works on x86-64, including under Rosetta).
//! 2. Patching a copy of the pages in a scratch mapping, and atomically
//!    remapping it over the original (required on Apple silicon).

use super::copy_code;
use crate::error::{MemoryError, Result};

#[allow(non_camel_case_types)]
type kern_return_t = libc::c_int;
#[allow(non_camel_case_types)]
type vm_prot_t = libc::c_int;

const KERN_SUCCESS: kern_return_t = 0;
const VM_PROT_READ: vm_prot_t = 0x01;
const VM_PROT_WRITE: vm_prot_t = 0x02;
const VM_PROT_EXECUTE: vm_prot_t = 0x04;
const VM_PROT_COPY: vm_prot_t = 0x10;
const VM_FLAGS_FIXED: libc::c_int = 0x0000;
const VM_FLAGS_ANYWHERE: libc::c_int = 0x0001;
const VM_FLAGS_OVERWRITE: libc::c_int = 0x4000;
const VM_INHERIT_COPY: libc::c_uint = 1;

unsafe extern "C" {
  static mach_task_self_: libc::mach_port_t;
  fn mach_vm_protect(
    task: libc::mach_port_t,
    address: u64,
    size: u64,
    set_maximum: libc::boolean_t,
    protection: vm_prot_t,
  ) -> kern_return_t;
  fn mach_vm_allocate(
    task: libc::mach_port_t,
    address: *mut u64,
    size: u64,
    flags: libc::c_int,
  ) -> kern_return_t;
  fn mach_vm_deallocate(task: libc::mach_port_t, address: u64, size: u64) -> kern_return_t;
  fn mach_vm_remap(
    target_task: libc::mach_port_t,
    target_address: *mut u64,
    size: u64,
    mask: u64,
    flags: libc::c_int,
    src_task: libc::mach_port_t,
    src_address: u64,
    copy: libc::boolean_t,
    cur_protection: *mut vm_prot_t,
    max_protection: *mut vm_prot_t,
    inheritance: libc::c_uint,
  ) -> kern_return_t;
}

fn check(result: kern_return_t) -> Result<()> {
  if result == KERN_SUCCESS {
    Ok(())
  } else {
    Err(MemoryError::mach(result).into())
  }
}

/// Overwrites code at `address` using Mach VM primitives.
///
/// # Safety
///
/// See [`super::patch_code`].
pub(super) unsafe fn patch_code(address: *mut u8, bytes: &[u8]) -> Result<()> {
  let page_size = region::page::size();
  let base = region::page::floor(address) as usize;
  let end = region::page::ceil(address.wrapping_add(bytes.len())) as usize;
  let size = (end - base) as u64;
  let offset = address as usize - base;

  // Preserve the protection of the page (typically `r-x`)
  let protection = region::query(address)?.protection();
  let mut native = 0;
  for (flag, value) in [
    (region::Protection::READ, VM_PROT_READ),
    (region::Protection::WRITE, VM_PROT_WRITE),
    (region::Protection::EXECUTE, VM_PROT_EXECUTE),
  ] {
    if protection.contains(flag) {
      native |= value;
    }
  }
  debug_assert!(size as usize % page_size == 0);

  // SAFETY: The task port of the current process is initialized by libSystem.
  let task = unsafe { mach_task_self_ };
  let rwx = VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE;

  // SAFETY: The pages are mapped (they were just queried), and copy-on-write
  // retains their contents.
  if unsafe { mach_vm_protect(task, base as u64, size, 0, rwx | VM_PROT_COPY) } == KERN_SUCCESS {
    // SAFETY: The range is now writable.
    unsafe { copy_code(address, bytes) };
    // SAFETY: Restores the original protection of the pages.
    return check(unsafe { mach_vm_protect(task, base as u64, size, 0, native) });
  }

  // Create a patched copy of the pages in a scratch mapping
  let mut scratch = 0u64;
  // SAFETY: The kernel chooses the address; the out-pointer is valid.
  check(unsafe { mach_vm_allocate(task, &mut scratch, size, VM_FLAGS_ANYWHERE) })?;

  let result = (|| {
    // SAFETY: Both ranges are mapped, readable, and `size` bytes long, and the
    // scratch mapping is writable.
    unsafe {
      copy_code(
        scratch as *mut u8,
        core::slice::from_raw_parts(base as *const u8, size as usize),
      );
      copy_code((scratch as usize + offset) as *mut u8, bytes);
    }
    // SAFETY: Changes the protection of our own scratch mapping.
    check(unsafe { mach_vm_protect(task, scratch, size, 0, native) })?;

    let mut target = base as u64;
    let (mut current, mut maximum) = (0, 0);
    // SAFETY: Atomically replaces the original pages with the patched copy,
    // sharing the scratch mapping's physical pages.
    check(unsafe {
      mach_vm_remap(
        task,
        &mut target,
        size,
        0,
        VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
        task,
        scratch,
        0,
        &mut current,
        &mut maximum,
        VM_INHERIT_COPY,
      )
    })
  })();

  // SAFETY: The scratch mapping is exclusively owned by this function; the
  // remapped pages remain referenced by the original range.
  unsafe { mach_vm_deallocate(task, scratch, size) };
  result
}
