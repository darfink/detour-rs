use cfg_if::cfg_if;

mod generic;
mod raw;

pub use self::generic::*;
pub use self::raw::*;

cfg_if! {
    if #[cfg(feature = "nightly")] {
        mod statik;
        pub use self::statik::*;
    } else {
    }
}
