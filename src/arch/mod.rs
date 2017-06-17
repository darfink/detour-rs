/// Architecture specific code
///
/// The current implementation requires a module to expose some functionality:
///
/// - A standalone `relay_builder` function.  
///   This function creates a relay for targets with large displacement, that
///   requires special attention. An example would be detours further away than 2GB
///   on x64. A relative jump is not enough, so the `relay_builder` generates an
///   absolute jump that the relative jump can reach. If it's needless, `None` can
///   be returned.
///
/// - A `Patcher`, modifies a target in-memory.
/// - A `Trampoline`, generates a callable address to the target.

cfg_if! {
    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        mod x86;
        pub use self::x86::{Patcher, Trampoline, relay_builder};
    } else {
        // Implement ARM support!
    }
}
