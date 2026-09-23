//! An allocator for executable memory in proximity to an address.
//!
//! Relative branches have a limited range (e.g. ±2 GiB on x86-64, ±128 MiB on
//! AArch64), so trampolines and relays must be allocated close to their
//! target. Memory is mapped in pools (one allocation granule each), which are
//! subdivided into fixed-size units.

use super::{copy_code, flush_instruction_cache};
use crate::error::{Error, MemoryError, Result};
use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

/// The allocation unit; also the alignment of each block.
const UNIT: usize = 16;

/// The maximum number of free regions attempted when mapping a new pool.
const MAX_CANDIDATES: usize = 64;

/// All pools of executable memory.
static POOLS: Mutex<Vec<Pool>> = Mutex::new(Vec::new());

/// A block of executable memory.
///
/// The memory is released once the block is dropped.
pub(crate) struct CodeBlock {
  address: usize,
  len: usize,
  mode: WriteMode,
}

impl CodeBlock {
  /// Returns a pointer to the start of the block.
  pub fn as_ptr(&self) -> *const u8 {
    self.address as *const u8
  }

  /// Returns the address of the block.
  pub fn address(&self) -> usize {
    self.address
  }

  /// Returns the capacity of the block, in bytes.
  #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
  pub fn len(&self) -> usize {
    self.len
  }

  /// Writes code to the start of the block.
  ///
  /// # Safety
  ///
  /// The block must not be executing whilst being written to, and the caller
  /// must hold the global patch lock (writes may temporarily change the
  /// protection of neighboring blocks).
  pub unsafe fn write(&mut self, code: &[u8]) -> Result<()> {
    assert!(code.len() <= self.len, "code exceeds its allocated block");
    let destination = self.address as *mut u8;

    match self.mode {
      // SAFETY: The block is mapped as read-write-execute.
      WriteMode::Direct => unsafe { copy_code(destination, code) },
      WriteMode::Toggle => {
        use region::Protection;
        // SAFETY: The block is exclusively owned, and not yet executing.
        unsafe {
          region::protect(destination, code.len(), Protection::READ_WRITE)
            .map_err(MemoryError::from_region)?;
          copy_code(destination, code);
          region::protect(destination, code.len(), Protection::READ_EXECUTE)
            .map_err(MemoryError::from_region)?;
        }
      },
      #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
      // SAFETY: `MAP_JIT` regions are writable for the current thread whilst
      // write protection is disabled. No other code is executed meanwhile.
      WriteMode::Jit => unsafe {
        libc::pthread_jit_write_protect_np(0);
        copy_code(destination, code);
        libc::pthread_jit_write_protect_np(1);
      },
    }

    flush_instruction_cache(destination, code.len());
    Ok(())
  }
}

impl Drop for CodeBlock {
  fn drop(&mut self) {
    let mut pools = POOLS.lock();
    let index = pools
      .iter()
      .position(|pool| pool.range().contains(&self.address))
      .expect("code block belongs to a pool");

    pools[index].release(self.address, self.len);
    if pools[index].is_empty() {
      // Unmaps the pool's memory
      pools.swap_remove(index);
    }
  }
}

impl fmt::Debug for CodeBlock {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "CodeBlock({:#x}..{:#x})",
      self.address,
      self.address + self.len
    )
  }
}

/// Allocates a block of executable memory, of at least `size` bytes, with its
/// entire range residing within `max_distance` bytes from `origin`.
pub(crate) fn allocate_near(origin: usize, max_distance: usize, size: usize) -> Result<CodeBlock> {
  let size = size.max(1).next_multiple_of(UNIT);
  let range = origin.saturating_sub(max_distance)..origin.saturating_add(max_distance);
  let mut pools = POOLS.lock();

  // Prefer existing pools, to limit the amount of mappings
  for pool in pools.iter_mut() {
    if let Some(address) = pool.allocate(size, &range) {
      return Ok(pool.block(address, size));
    }
  }

  let granularity = os::granularity();
  let pool_size = size.next_multiple_of(granularity);

  for candidate in free_candidates(origin, &range, pool_size, granularity) {
    // The kernel may choose another address, which may still be in range
    let Some(mapping) = os::Mapping::new(candidate, pool_size) else {
      continue;
    };

    if range.start <= mapping.base && mapping.base + mapping.len <= range.end {
      let mut pool = Pool::new(mapping);
      let address = pool
        .allocate(size, &range)
        .expect("allocating from a new pool");
      let block = pool.block(address, size);
      pools.push(pool);
      return Ok(block);
    }
  }

  Err(Error::NoNearbyMemory)
}

/// Returns addresses of unmapped memory suitable for a pool, ordered by their
/// distance to `origin`.
fn free_candidates(
  origin: usize,
  range: &Range<usize>,
  size: usize,
  granularity: usize,
) -> Vec<usize> {
  // Never attempt allocating the null page(s)
  let lower = range.start.max(granularity);
  let upper = range.end;
  if lower >= upper {
    return Vec::new();
  }

  // Unmapped memory is the gaps between mapped regions
  let mut gaps = Vec::new();
  let mut cursor = lower;
  let mut complete = true;

  match region::query_range(lower as *const u8, upper - lower) {
    Ok(regions) => {
      for region in regions {
        let Ok(region) = region else {
          complete = false;
          break;
        };
        let region = region.as_range();
        if region.start > cursor {
          gaps.push(cursor..region.start.min(upper));
        }
        cursor = cursor.max(region.end);
      }
    },
    Err(_) => complete = false,
  }

  if complete && cursor < upper {
    gaps.push(cursor..upper);
  }

  let mut candidates: Vec<usize> = gaps
    .into_iter()
    .filter_map(|gap| {
      let first = gap.start.checked_next_multiple_of(granularity)?;
      let last = gap.end.checked_sub(size)? / granularity * granularity;
      (first <= last).then(|| (origin / granularity * granularity).clamp(first, last))
    })
    .filter(|&address| range.start <= address && address.saturating_add(size) <= range.end)
    .collect();

  candidates.sort_by_key(|&address| address.abs_diff(origin));
  candidates.truncate(MAX_CANDIDATES);
  candidates
}

/// Describes how the memory of a pool is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
  /// The memory is mapped as read-write-execute.
  #[cfg_attr(
    all(target_vendor = "apple", target_arch = "aarch64"),
    allow(dead_code)
  )]
  Direct,
  /// The memory is read-execute, and temporarily made read-write (W^X).
  Toggle,
  /// The memory is mapped with `MAP_JIT` (Apple silicon).
  #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
  Jit,
}

/// A mapping subdivided into units.
struct Pool {
  mapping: os::Mapping,
  used: Vec<bool>,
  live: usize,
}

impl Pool {
  fn new(mapping: os::Mapping) -> Self {
    let units = mapping.len / UNIT;
    Pool {
      mapping,
      used: vec![false; units],
      live: 0,
    }
  }

  fn range(&self) -> Range<usize> {
    self.mapping.base..self.mapping.base + self.mapping.len
  }

  fn is_empty(&self) -> bool {
    self.live == 0
  }

  fn block(&self, address: usize, len: usize) -> CodeBlock {
    CodeBlock {
      address,
      len,
      mode: self.mapping.mode,
    }
  }

  /// Allocates `size` bytes (a multiple of `UNIT`) within `range`.
  fn allocate(&mut self, size: usize, range: &Range<usize>) -> Option<usize> {
    let count = size / UNIT;
    let base = self.mapping.base;
    let mut run = 0;

    for index in 0..self.used.len() {
      run = if self.used[index] { 0 } else { run + 1 };
      if run < count {
        continue;
      }

      let first = index + 1 - count;
      let address = base + first * UNIT;
      if range.start <= address && address + size <= range.end {
        self.used[first..=index].fill(true);
        self.live += 1;
        return Some(address);
      }
    }

    None
  }

  fn release(&mut self, address: usize, size: usize) {
    let first = (address - self.mapping.base) / UNIT;
    let units = &mut self.used[first..first + size / UNIT];
    debug_assert!(units.iter().all(|used| *used));
    units.fill(false);
    self.live -= 1;
  }
}

#[cfg(unix)]
mod os {
  use super::WriteMode;
  use libc::{MAP_ANON, MAP_FAILED, MAP_PRIVATE, PROT_EXEC, PROT_READ, PROT_WRITE};

  pub fn granularity() -> usize {
    region::page::size()
  }

  /// An anonymous memory mapping.
  pub struct Mapping {
    pub base: usize,
    pub len: usize,
    pub mode: WriteMode,
  }

  // SAFETY: A mapping is plain memory; it is not tied to a thread.
  unsafe impl Send for Mapping {}

  impl Mapping {
    /// Maps memory, preferably at `address`.
    ///
    /// `address` is only provided as a hint (i.e. `MAP_FIXED` is not used),
    /// so existing mappings are never replaced.
    pub fn new(address: usize, len: usize) -> Option<Self> {
      #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
      let attempts = [
        (
          PROT_READ | PROT_WRITE | PROT_EXEC,
          libc::MAP_JIT,
          WriteMode::Jit,
        ),
        (PROT_READ | PROT_WRITE, 0, WriteMode::Toggle),
      ];
      #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
      let attempts = [
        (PROT_READ | PROT_WRITE | PROT_EXEC, 0, WriteMode::Direct),
        (PROT_READ | PROT_WRITE, 0, WriteMode::Toggle),
      ];

      for (protection, flags, mode) in attempts {
        // SAFETY: Mapping anonymous memory without `MAP_FIXED` cannot affect
        // existing mappings.
        let base = unsafe {
          libc::mmap(
            address as *mut libc::c_void,
            len,
            protection,
            MAP_PRIVATE | MAP_ANON | flags,
            -1,
            0,
          )
        };

        if base == MAP_FAILED {
          // The requested protection may be disallowed (e.g. W^X policies)
          continue;
        }

        return Some(Mapping {
          base: base as usize,
          len,
          mode,
        });
      }

      None
    }
  }

  impl Drop for Mapping {
    fn drop(&mut self) {
      // SAFETY: The mapping is exclusively owned.
      unsafe { libc::munmap(self.base as *mut libc::c_void, self.len) };
    }
  }
}

#[cfg(windows)]
mod os {
  use super::WriteMode;
  use region::Protection;

  /// `VirtualAlloc` reserves memory at 64 KiB boundaries.
  pub fn granularity() -> usize {
    0x10000
  }

  /// A virtual memory allocation.
  pub struct Mapping {
    pub base: usize,
    pub len: usize,
    pub mode: WriteMode,
    _allocation: region::Allocation,
  }

  // SAFETY: A mapping is plain memory; it is not tied to a thread.
  unsafe impl Send for Mapping {}

  impl Mapping {
    /// Allocates memory at exactly `address`, if the range is available.
    pub fn new(address: usize, len: usize) -> Option<Self> {
      for (protection, mode) in [
        (Protection::READ_WRITE_EXECUTE, WriteMode::Direct),
        (Protection::READ_WRITE, WriteMode::Toggle),
      ] {
        // `VirtualAlloc` fails, rather than replaces, if the range is in use
        let Ok(allocation) = region::alloc_at(address as *const u8, len, protection) else {
          continue;
        };

        let mapping = Mapping {
          base: allocation.as_ptr::<u8>() as usize,
          len: allocation.len(),
          mode,
          _allocation: allocation,
        };
        return (mapping.base == address).then_some(mapping);
      }

      None
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// The branch range of the host architecture.
  const DISTANCE: usize = if cfg!(target_arch = "aarch64") {
    0x07F0_0000
  } else {
    0x7FF0_0000
  };

  #[test]
  fn allocates_within_range() -> Result<()> {
    let origin = allocates_within_range as *const () as usize;
    let max_distance = DISTANCE;

    let blocks = (0..64)
      .map(|_| allocate_near(origin, max_distance, 40))
      .collect::<Result<Vec<_>>>()?;

    for block in &blocks {
      assert_eq!(block.len(), 48);
      assert_eq!(block.address() % UNIT, 0);
      assert!(block.address().abs_diff(origin) < max_distance);
      assert!((block.address() + block.len()).abs_diff(origin) < max_distance);
    }

    // No two blocks may overlap
    let mut ranges: Vec<_> = blocks
      .iter()
      .map(|b| b.address()..b.address() + b.len())
      .collect();
    ranges.sort_by_key(|range| range.start);
    assert!(ranges.windows(2).all(|pair| pair[0].end <= pair[1].start));
    Ok(())
  }

  #[test]
  fn allocated_code_is_executable() -> Result<()> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    let code: &[u8] = &[0xB8, 0x2A, 0x00, 0x00, 0x00, 0xC3]; // mov eax, 42; ret
    #[cfg(target_arch = "aarch64")]
    let code: &[u8] = &[0x40, 0x05, 0x80, 0x52, 0xC0, 0x03, 0x5F, 0xD6]; // mov w0, #42; ret

    let origin = allocated_code_is_executable as *const () as usize;
    let mut block = allocate_near(origin, DISTANCE, code.len())?;
    let _lock = super::super::patch_lock();
    // SAFETY: The block is not yet executing.
    unsafe { block.write(code)? };

    // SAFETY: The block contains a function matching the signature.
    let function: extern "C" fn() -> i32 = unsafe { core::mem::transmute(block.as_ptr()) };
    assert_eq!(function(), 42);
    Ok(())
  }

  #[test]
  fn candidates_are_sorted_by_distance() {
    let origin = candidates_are_sorted_by_distance as *const () as usize;
    let granularity = os::granularity();
    let range = origin.saturating_sub(1 << 30)..origin.saturating_add(1 << 30);
    let candidates = free_candidates(origin, &range, granularity, granularity);

    assert!(!candidates.is_empty());
    assert!(candidates.iter().all(|address| address % granularity == 0));
    assert!(
      candidates
        .windows(2)
        .all(|pair| pair[0].abs_diff(origin) <= pair[1].abs_diff(origin))
    );
  }
}
