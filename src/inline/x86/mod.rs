extern crate libudis86_sys as udis;

// Re-exports
pub use self::patcher::Patcher;
pub use self::trampoline::Trampoline;

// Modules
mod patcher;
mod trampoline;
mod thunk;
