//! The detour types.

mod context;
mod raw;
mod reentrancy;
pub(crate) mod statik;
mod typed;

pub use self::context::{Context, ContextDetour};
pub use self::raw::RawDetour;
pub use self::typed::{Original, TypedDetour};
