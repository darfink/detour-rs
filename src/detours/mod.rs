mod raw;
mod statik;
mod typed;

pub use self::raw::RawDetour;
pub use self::statik::StaticDetour;
pub use self::typed::TypedDetour;

/// Renamed to [`TypedDetour`].
#[deprecated(since = "0.10.0", note = "renamed to `TypedDetour`")]
pub type GenericDetour<T> = TypedDetour<T>;
