/// Macro for defining type-safe detours.
///
/// This macro uses `RawDetour` for its implementation, but it exposes its
/// functionality in a type-safe wrapper.  
/// The macro can contain one or more definitions — each definition will become a
/// distinct type.
///
/// A type may only have one active detour at a time. Therefore another
/// `initialize` call can only be done once the previous detour has been dropped.
/// This is due to the closures being stored as static variables.
///
/// # Example
///
/// ```rust
/// # #[macro_use] extern crate lazy_static;
/// # #[macro_use] extern crate detour;
/// # fn main() { unsafe { example() } }
/// static_detours! {
///     struct Test: /* [extern "X"] */ fn(i32) -> i32;
/// }
///
/// fn add5(val: i32) -> i32 { val + 5 }
/// fn add10(val: i32) -> i32 { val + 10 }
///
/// unsafe fn example() {
///     // The detour can also be a closure
///     let mut hook = Test::initialize(add5, add10).unwrap();
///
///     assert_eq!(add5(5), 10);
///     assert_eq!(hook.call(5), 10);
///
///     hook.enable().unwrap();
///
///     assert_eq!(add5(5), 15);
///     assert_eq!(hook.call(5), 10);
///
///     hook.disable().unwrap();
///
///     assert_eq!(add5(5), 10);
/// }
/// ```
///
/// Any type of function is supported, and `extern` is optional.
///
/// There is also an example module [available](example/index.html).
///
/// **NOTE: Requires [lazy_static](https://crates.io/crates/lazy_static).**
#[macro_export]
// Inspired by: https://github.com/Jascha-N/minhook-rs
macro_rules! static_detours {
    // 1 — meta attributes
    // See: https://github.com/rust-lang/rust/issues/24189
    (@parse_attributes ($($input:tt)*) | $(#[$attribute:meta])* $next:tt $($rest:tt)*) => {
        static_detours!(@parse_access_modifier ($($input)* ($($attribute)*)) | $next $($rest)*);
    };

    // 2 — pub modifier (yes/no)
    (@parse_access_modifier ($($input:tt)*) | pub struct $($rest:tt)*) => {
        static_detours!(@parse_name ($($input)* (pub)) | $($rest)*);
    };
    (@parse_access_modifier ($($input:tt)*) | struct $($rest:tt)*) => {
        static_detours!(@parse_name ($($input)* ()) | $($rest)*);
    };

    // 3 — detour name
    (@parse_name ($($input:tt)*) | $name:ident : $($rest:tt)*) => {
        static_detours!(@parse_unsafe ($($input)* ($name)) | $($rest)*);
    };

    // 4 — unsafe modifier (yes/no)
    (@parse_unsafe ($($input:tt)*) | unsafe $($rest:tt)*) => {
        static_detours!(@parse_calling_convention ($($input)*) (unsafe) | $($rest)*);
    };
    (@parse_unsafe ($($input:tt)*) | $($rest:tt)*) => {
        static_detours!(@parse_calling_convention ($($input)*) () | $($rest)*);
    };

    // 5 — calling convention (extern "XXX"/extern/-)
    (@parse_calling_convention ($($input:tt)*) ($($modifier:tt)*) | extern $cc:tt fn $($rest:tt)*) => {
        static_detours!(@parse_prototype ($($input)* ($($modifier)* extern $cc)) | $($rest)*);
    };
    (@parse_calling_convention ($($input:tt)*) ($($modifier:tt)*) | extern fn $($rest:tt)*) => {
        static_detours!(@parse_prototype ($($input)* ($($modifier)* extern)) | $($rest)*);
    };
    (@parse_calling_convention ($($input:tt)*) ($($modifier:tt)*) | fn $($rest:tt)*) => {
        static_detours!(@parse_prototype ($($input)* ($($modifier)*)) | $($rest)*);
    };

    // 6 — argument and return type (return/void)
    (@parse_prototype ($($input:tt)*) | ($($argument_type:ty),*) -> $return_type:ty ; $($rest:tt)*) => {
        static_detours!(@parse_terminator ($($input)* ($($argument_type)*) ($return_type)) | ; $($rest)*);
    };
    (@parse_prototype ($($input:tt)*) | ($($argument_type:ty),*) $($rest:tt)*) => {
        static_detours!(@parse_terminator ($($input)* ($($argument_type)*) (())) | $($rest)*);
    };

    // 7 — semicolon terminator
    (@parse_terminator ($($input:tt)*) | ; $($rest:tt)*) => {
        static_detours!(@parse_entries ($($input)*) | $($rest)*);
    };

    // 8 - additional detours (multiple/single)
    (@parse_entries ($($input:tt)*) | $($rest:tt)+) => {
        static_detours!(@aggregate $($input)*);
        static_detours!($($rest)*);
    };
    (@parse_entries ($($input:tt)*) | ) => {
        static_detours!(@aggregate $($input)*);
    };

    // 9 - aggregate data for the generate function
    (@aggregate ($($attribute:meta)*) ($($visibility:tt)*) ($name:ident)
                ($($modifier:tt)*) ($($argument_type:ty)*) ($return_type:ty)) => {
        static_detours!(@argument_names (create_detour)(
            ($($attribute)*) ($($visibility)*) ($name)
            ($($modifier)*) ($($argument_type)*) ($return_type)
            ($($modifier)* fn ($($argument_type),*) -> $return_type)
        )($($argument_type)*));
    };

    // 10 - detour type implementation
    // TODO: Support access to detour in closure
    (@create_detour ($($argument_name:ident)*) ($($attribute:meta)*) ($($visibility:tt)*)
                    ($name:ident) ($($modifier:tt)*) ($($argument_type:ty)*)
                    ($return_type:ty) ($fn_type:ty)) => {
        static_detours!(@generate
            $(#[$attribute])*
            $($visibility)* struct $name($crate::RawDetour);
        );
        static_detours!(@generate
            impl $name {
                /// Constructs a new detour for the target, and initializes the
                /// static mutex with the supplied closure.
                pub unsafe fn initialize<T>(target: $fn_type, closure: T)
                        -> $crate::error::Result<Self> where
                        T: Fn($($argument_type),*) -> $return_type + Send + 'static {
                    let mut static_closure = Self::closure().lock().unwrap();
                    if static_closure.is_some() {
                        Err($crate::error::ErrorKind::AlreadyExisting.into())
                    } else {
                        let detour = $crate::RawDetour::new(
                            target as *const (),
                            Self::callback as *const ())?;
                        *static_closure = Some(Box::new(closure));
                        Ok($name(detour))
                    }
                }

                #[allow(dead_code)]
                /// Calls the original function regardless whether the function
                /// is hooked or not.
                pub unsafe fn call(&self, $($argument_name: $argument_type),*) -> $return_type {
                    let trampoline: $fn_type = ::std::mem::transmute(self.0.trampoline());
                    trampoline($($argument_name),*)
                }

                #[allow(dead_code)]
                /// Enables the detour
                pub unsafe fn enable(&mut self) -> $crate::error::Result<()> {
                    self.0.enable()
                }

                #[allow(dead_code)]
                /// Disables the detour
                pub unsafe fn disable(&mut self) -> $crate::error::Result<()> {
                    self.0.disable()
                }

                #[allow(dead_code)]
                /// Returns whether the detour is enabled or not.
                pub fn is_enabled(&self) -> bool {
                    self.0.is_enabled()
                }

                #[doc(hidden)]
                #[allow(dead_code)]
                $($modifier) * fn callback($($argument_name: $argument_type),*) -> $return_type {
                    Self::closure().lock().unwrap()
                        .as_ref().expect("calling detour closure; is null")($($argument_name),*)
                }

                fn closure() -> &'static ::std::sync::Mutex<Option<Box<Fn($($argument_type),*)
                        -> $return_type + Send>>> {
                    lazy_static! {
                        static ref CLOSURE:
                            ::std::sync::Mutex<Option<Box<Fn($($argument_type),*) -> $return_type + Send>>> =
                                ::std::sync::Mutex::new(None);
                    }

                    &*CLOSURE
                }
            }
        );
        static_detours!(@generate
            impl Drop for $name {
                /// Disables the detour and frees the associated closure.
                fn drop(&mut self) {
                    unsafe {
                        self.0.disable().expect("disabling detour on drop");
                        *Self::closure().lock().unwrap() = None;
                    }
                }
            }
        );
    };

    // Associates each argument type with a dummy name.
    (@argument_names ($label:ident) ($($input:tt)*) ($($token:tt)*)) => {
        static_detours!(@argument_names ($label) ($($input)*)(
            __arg_0  __arg_1  __arg_2  __arg_3  __arg_4  __arg_5  __arg_6  __arg_7
            __arg_8  __arg_9  __arg_10 __arg_11 __arg_12 __arg_13 __arg_14 __arg_15
        )($($token)*)());
    };
    (@argument_names ($label:ident) ($($input:tt)*) ($hd_name:tt $($tl_name:tt)*) ($hd:tt $($tl:tt)*) ($($acc:tt)*) ) => {
        static_detours!(@argument_names ($label) ($($input)*) ($($tl_name)*) ($($tl)*) ($($acc)* $hd_name));
    };
    (@argument_names ($label:ident) ($($input:tt)*) ($($name:tt)*) () ($($acc:tt)*)) => {
        static_detours!(@$label ($($acc)*) $($input)*);
    };

    (@generate $item:item) => { $item };

    // Bootstrapper
    ($($t:tt)+) => {
        static_detours!(@parse_attributes () | $($t)+);
    };
}
