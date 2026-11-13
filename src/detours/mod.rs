//! The detour types.

mod mid;
mod raw;
mod reentrancy;
pub(crate) mod statik;
mod typed;

pub use self::mid::{Context, MidDetour};
pub use self::raw::RawDetour;
pub use self::typed::{Original, TypedDetour};
