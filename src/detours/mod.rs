//! The detour types.

mod raw;
mod reentrancy;
pub(crate) mod statik;
mod typed;

pub use self::raw::RawDetour;
pub use self::typed::{Original, TypedDetour};
