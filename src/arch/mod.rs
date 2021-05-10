/// Architecture specific code
///
/// The current implementation requires a module to expose some functionality:
///
/// - A standalone `relay_builder` function.
/// This function creates a relay for targets with large displacement, that
/// requires special attention. An example would be detours further away than
/// 2GB on x64. A relative jump is not enough, so the `relay_builder`
/// generates an absolute jump that the relative jump can reach. If it's
/// needless, `None` can   be returned.
///
/// - A `Patcher`, modifies a target in-memory.
/// - A `Trampoline`, generates a callable address to the target.
pub use self::detour::Detour;

use cfg_if::cfg_if;

// TODO: flush instruction cache? __clear_cache
// See: https://github.com/llvm-mirror/compiler-rt/blob/master/lib/builtins/clear_cache.c
cfg_if! {
    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        mod x86;
        use self::x86::{Patcher, Trampoline, meta};
    } else {
        // TODO: Implement ARM/AARCH64/MIPS support!
    }
}

mod detour;
mod memory;

/// Returns true if the displacement is within a certain range.
pub fn is_within_range(displacement: isize) -> bool {
  let range = meta::DETOUR_RANGE as i64;
  (-range..range).contains(&(displacement as i64))
}
