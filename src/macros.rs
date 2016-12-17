macro_rules! static_hooks {
    ($name:ident : extern $linkage:tt fn ($($arg:ty),*) -> $return_type:ty ;) => {
        use std::sync::atomic::{AtomicPtr, Ordering};

        static CALLBACK: AtomicPtr<> = AtomicPtr::new(0 as *mut _);
        struct $name($crate::InlineDetour);

        impl $name {
            unsafe fn new<T>(target: extern $linkage fn($($arg),*) -> $return_type, callback: T)
                             -> $crate::error::Result<$name> where T: Fn($($arg),*) -> $return_type {
                CALLBACK.load(Box::into_raw(Box::new(callback)), Ordering::SeqCst);
                Ok($name($crate::InlineDetour::new(target as *const (), Self::hook as *const ())?))
            }

            extern $linkage fn hook($(_: $arg),*) -> $return_type {
                0
            }
        }
    }
}
