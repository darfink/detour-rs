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
/// # use detour::Detour;
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
            $($visibility)* struct $name(());
        );
        static_detours!(@generate
            impl $name {
                /// Constructs a new detour for the target, and initializes the
                /// static mutex with the supplied closure.
                pub unsafe fn initialize<T>(target: $fn_type, closure: T) ->
                        $crate::error::Result<$crate::GenericDetour<$fn_type>>
                        where T: Fn($($argument_type),*) -> $return_type + Send + 'static {
                    let mut static_closure = Self::closure().lock().unwrap();
                    if static_closure.is_some() {
                        Err($crate::error::ErrorKind::AlreadyExisting.into())
                    } else {
                        let detour = $crate::GenericDetour::<$fn_type>::new(target, Self::callback)?;
                        *static_closure = Some(Box::new(closure));
                        Ok(detour)
                    }
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
        //static_detours!(@generate
        //    impl Drop for $name {
        //        /// Disables the detour and frees the associated closure.
        //        fn drop(&mut self) {
        //            unsafe {
        //                self.0.disable().expect("disabling detour on drop");
        //                *Self::closure().lock().unwrap() = None;
        //            }
        //        }
        //    }
        //);
    };

    // Associates each argument type with a dummy name.
    (@argument_names ($label:ident) ($($input:tt)*) ($($token:tt)*)) => {
        static_detours!(@argument_names ($label) ($($input)*)(
            __arg_0  __arg_1  __arg_2  __arg_3  __arg_4  __arg_5  __arg_6
            __arg_7  __arg_8  __arg_9  __arg_10 __arg_11 __arg_12 __arg_13
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

macro_rules! impl_hookable {
    (@recurse () ($($nm:ident : $ty:ident),*)) => {
        impl_hookable!(@impl_all ($($nm : $ty),*));
    };
    (@recurse ($hd_nm:ident : $hd_ty:ident $(, $tl_nm:ident : $tl_ty:ident)*) ($($nm:ident : $ty:ident),*)) => {
        impl_hookable!(@impl_all ($($nm : $ty),*));
        impl_hookable!(@recurse ($($tl_nm : $tl_ty),*) ($($nm : $ty,)* $hd_nm : $hd_ty));
    };

    (@impl_all ($($nm:ident : $ty:ident),*)) => {
        impl_hookable!(@impl_pair ($($nm : $ty),*) (                  fn($($ty),*) -> Ret));
        impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "cdecl"    fn($($ty),*) -> Ret));
        impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "stdcall"  fn($($ty),*) -> Ret));
        impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "fastcall" fn($($ty),*) -> Ret));
        impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "win64"    fn($($ty),*) -> Ret));
        impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "C"        fn($($ty),*) -> Ret));
        impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "system"   fn($($ty),*) -> Ret));
    };

    (@impl_pair ($($nm:ident : $ty:ident),*) ($($fn_t:tt)*)) => {
        impl_hookable!(@impl_fun ($($nm : $ty),*) ($($fn_t)*) (unsafe $($fn_t)*));
    };

    (@impl_fun ($($nm:ident : $ty:ident),*) ($safe_type:ty) ($unsafe_type:ty)) => {
        impl_hookable!(@impl_core ($($nm : $ty),*) ($safe_type));
        impl_hookable!(@impl_core ($($nm : $ty),*) ($unsafe_type));

        impl_hookable!(@impl_hookable_with ($($nm : $ty),*) ($unsafe_type) ($safe_type));
        impl_hookable!(@impl_safe ($($nm : $ty),*) ($safe_type));
    };

    (@impl_hookable_with ($($nm:ident : $ty:ident),*) ($target:ty) ($detour:ty)) => {
        unsafe impl<Ret: 'static, $($ty: 'static),*> HookableWith<$detour> for $target {}
    };

    (@impl_safe ($($nm:ident : $ty:ident),*) ($fn_type:ty)) => {
        impl<Ret: 'static, $($ty: 'static),*> $crate::GenericDetour<$fn_type> {
            #[doc(hidden)]
            pub fn call(&self, $($nm : $ty),*) -> Ret {
                unsafe {
                    let original: $fn_type = ::std::mem::transmute(self.trampoline());
                    original($($nm),*)
                }
            }
        }
    };

    (@impl_core ($($nm:ident : $ty:ident),*) ($fn_type:ty)) => {
        unsafe impl<Ret: 'static, $($ty: 'static),*> Function for $fn_type {
            type Arguments = ($($ty,)*);
            type Output = Ret;

            unsafe fn from_ptr(ptr: *const ()) -> Self {
                ::std::mem::transmute(ptr)
            }

            fn to_ptr(&self) -> *const () {
                unsafe { ::std::mem::transmute(*self) }
            }
        }
    };

    ($($nm:ident : $ty:ident),*) => {
        impl_hookable!(@recurse ($($nm : $ty),*) ());
    };
}
