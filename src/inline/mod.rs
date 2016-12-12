// Re-export the inline detour
pub use self::detour::InlineDetour as Inline;

// Modules
mod detour;
mod alloc;
mod pic;

cfg_if! {
    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        mod x86;
        use self::x86 as arch;
    } else {
        // Implement ARM support!
    }
}

