// Re-export the inline detour
pub use self::detour::InlineDetour as Inline;

// Modules
mod detour;
mod pic;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use self::x86 as arch;

