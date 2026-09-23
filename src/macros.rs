/// Applies a `cfg` of the supported architectures to each item.
macro_rules! supported {
  ($($item:item)*) => {
    $(
      #[cfg(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))]
      $item
    )*
  };
}

/// A macro for defining static, type-safe detours.
///
/// This macro defines one or more [`StaticDetour`](crate::StaticDetour)s.
///
/// # Syntax
///
/// ```ignore
/// static_detour! {
///   [pub] static NAME_1: [unsafe] [extern "cc"] fn([argument]...) [-> ret];
///   [pub] static NAME_2: [unsafe] [extern "cc"] fn([argument]...) [-> ret];
///   ...
///   [pub] static NAME_N: [unsafe] [extern "cc"] fn([argument]...) [-> ret];
/// }
/// ```
///
/// # Example
///
/// ```rust
/// # use detour::static_detour;
/// static_detour! {
///   // The simplest detour
///   static Foo: fn();
///
///   // An unsafe public detour with a different calling convention
///   pub static PubFoo: unsafe extern "C" fn(i32) -> i32;
///
///   // A specific visibility modifier, and a trailing comma
///   pub(crate) static PubSelf: unsafe extern "C" fn(i32, i32,);
/// }
/// ```
#[macro_export]
// Inspired by: https://github.com/Jascha-N/minhook-rs
macro_rules! static_detour {
  () => {};

  // Normalize the function qualifiers
  ($(#[$attr:meta])* $vis:vis static $name:ident : unsafe extern $abi:literal fn $($rest:tt)*) => {
    $crate::static_detour!(@signature [$(#[$attr])*] [$vis] [$name] [unsafe extern $abi] $($rest)*);
  };
  ($(#[$attr:meta])* $vis:vis static $name:ident : unsafe extern fn $($rest:tt)*) => {
    $crate::static_detour!(@signature [$(#[$attr])*] [$vis] [$name] [unsafe extern "C"] $($rest)*);
  };
  ($(#[$attr:meta])* $vis:vis static $name:ident : unsafe fn $($rest:tt)*) => {
    $crate::static_detour!(@signature [$(#[$attr])*] [$vis] [$name] [unsafe] $($rest)*);
  };
  ($(#[$attr:meta])* $vis:vis static $name:ident : extern $abi:literal fn $($rest:tt)*) => {
    $crate::static_detour!(@signature [$(#[$attr])*] [$vis] [$name] [extern $abi] $($rest)*);
  };
  ($(#[$attr:meta])* $vis:vis static $name:ident : extern fn $($rest:tt)*) => {
    $crate::static_detour!(@signature [$(#[$attr])*] [$vis] [$name] [extern "C"] $($rest)*);
  };
  ($(#[$attr:meta])* $vis:vis static $name:ident : fn $($rest:tt)*) => {
    $crate::static_detour!(@signature [$(#[$attr])*] [$vis] [$name] [] $($rest)*);
  };

  // Parse the arguments and the (optional) return type
  (@signature $attrs:tt $vis:tt $name:tt $qualifiers:tt
      ($($argument:ty),* $(,)?) -> $output:ty ; $($rest:tt)*) => {
    $crate::static_detour!(@names $attrs $vis $name $qualifiers [$output] [] [$($argument),*]
      [__arg_0 __arg_1 __arg_2 __arg_3 __arg_4 __arg_5 __arg_6
       __arg_7 __arg_8 __arg_9 __arg_10 __arg_11 __arg_12 __arg_13]);
    $crate::static_detour!($($rest)*);
  };
  (@signature $attrs:tt $vis:tt $name:tt $qualifiers:tt ($($argument:ty),* $(,)?) ; $($rest:tt)*) => {
    $crate::static_detour!(@names $attrs $vis $name $qualifiers [()] [] [$($argument),*]
      [__arg_0 __arg_1 __arg_2 __arg_3 __arg_4 __arg_5 __arg_6
       __arg_7 __arg_8 __arg_9 __arg_10 __arg_11 __arg_12 __arg_13]);
    $crate::static_detour!($($rest)*);
  };

  // Associate each argument type with a name
  (@names [$($attr:tt)*] [$vis:vis] [$name:ident] [$($qualifier:tt)*] [$output:ty]
      [$(($argument_name:ident: $argument:ty))*] [] [$($unused:ident)*]) => {
    $($attr)*
    #[allow(non_upper_case_globals)]
    $vis static $name: $crate::StaticDetour<$($qualifier)* fn($($argument),*) -> $output> = {
      #[inline(never)]
      #[allow(unused_unsafe)]
      $($qualifier)* fn __ffi_detour($($argument_name: $argument),*) -> $output {
        $name.__with_detour(move |detour| detour($($argument_name),*))
      }

      $crate::StaticDetour::__new(__ffi_detour)
    };
  };
  (@names $attrs:tt $vis:tt $name:tt $qualifiers:tt $output:tt
      [$($named:tt)*] [$argument:ty $(, $arguments:ty)*] [$next:ident $($names:ident)*]) => {
    $crate::static_detour!(@names $attrs $vis $name $qualifiers $output
      [$($named)* ($next: $argument)] [$($arguments),*] [$($names)*]);
  };
}

/// Implements `Function`, `HookableWith`, and the signature-specific methods
/// of `TypedDetour` & `StaticDetour`, for all supported function pointers.
macro_rules! impl_hookable {
  (@recurse () ($($nm:ident : $ty:ident),*)) => {
    impl_hookable!(@impl_all ($($nm : $ty),*));
  };
  (@recurse
      ($hd_nm:ident : $hd_ty:ident $(, $tl_nm:ident : $tl_ty:ident)*)
      ($($nm:ident : $ty:ident),*)) => {
    impl_hookable!(@impl_all ($($nm : $ty),*));
    impl_hookable!(@recurse ($($tl_nm : $tl_ty),*) ($($nm : $ty,)* $hd_nm : $hd_ty));
  };

  // The signature-specific methods are only documented once, for `fn(A) -> Ret`,
  // to avoid hundreds of near-identical entries in the documentation.
  (@impl_all (__arg_0 : A)) => {
    impl_hookable!(@impl_abis [] (__arg_0 : A));
  };
  (@impl_all ($($nm:ident : $ty:ident),*)) => {
    impl_hookable!(@impl_abis [#[doc(hidden)]] ($($nm : $ty),*));
  };

  (@impl_abis [$($doc:tt)*] ($($nm:ident : $ty:ident),*)) => {
    impl_hookable!(@impl_pair [$($doc)*]         ($($nm : $ty),*) (                       fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "C"             fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "C-unwind"      fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "system"        fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "system-unwind" fn($($ty),*) -> Ret));

    #[cfg(target_arch = "x86")]
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "cdecl"         fn($($ty),*) -> Ret));
    #[cfg(target_arch = "x86")]
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "stdcall"       fn($($ty),*) -> Ret));
    #[cfg(target_arch = "x86")]
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "fastcall"      fn($($ty),*) -> Ret));
    #[cfg(target_arch = "x86")]
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "thiscall"      fn($($ty),*) -> Ret));

    #[cfg(target_arch = "x86_64")]
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "win64"         fn($($ty),*) -> Ret));
    #[cfg(target_arch = "x86_64")]
    impl_hookable!(@impl_pair [#[doc(hidden)]] ($($nm : $ty),*) (extern "sysv64"        fn($($ty),*) -> Ret));
  };

  (@impl_pair [$($doc:tt)*] ($($nm:ident : $ty:ident),*) ($($fn_t:tt)*)) => {
    impl_hookable!(@impl_fun [$($doc)*] ($($nm : $ty),*) ($($fn_t)*) (unsafe $($fn_t)*));
  };

  (@impl_fun [$($doc:tt)*] ($($nm:ident : $ty:ident),*) ($safe_type:ty) ($unsafe_type:ty)) => {
    impl_hookable!(@impl_core [$($doc)*] ($($nm : $ty),*) ($safe_type) ());
    impl_hookable!(@impl_core [#[doc(hidden)]] ($($nm : $ty),*) ($unsafe_type) (unsafe));

    // SAFETY: A safe function can be used wherever an unsafe one is expected.
    unsafe impl<Ret: 'static, $($ty: 'static),*> HookableWith<$safe_type> for $unsafe_type {}
  };

  (@impl_core [$($doc:tt)*] ($($nm:ident : $ty:ident),*) ($fn_type:ty) ($($unsafety:tt)?)) => {
    // SAFETY: Implemented for function pointers only.
    unsafe impl<Ret: 'static, $($ty: 'static),*> Function for $fn_type {
      type Arguments = ($($ty,)*);
      type Output = Ret;
      type Closure = dyn Fn($($ty),*) -> Ret + Send + Sync;

      unsafe fn from_ptr(ptr: *const ()) -> Self {
        // SAFETY: Function pointers and data pointers share representation on
        // all supported platforms; validity is guaranteed by the caller.
        unsafe { ::core::mem::transmute::<*const (), Self>(ptr) }
      }

      fn to_ptr(&self) -> *const () {
        *self as *const ()
      }
    }

    $($doc)*
    impl<Ret: 'static, $($ty: 'static),*> $crate::TypedDetour<$fn_type> {
      /// Calls the original function, regardless of whether it is detoured or
      /// not.
      ///
      /// Available for all supported signatures, taking the same arguments
      /// as the target. It is `unsafe` if the target is.
      pub $($unsafety)? fn call(&self, $($nm : $ty),*) -> Ret {
        // SAFETY: The trampoline shares the target's signature, and remains
        // valid for the lifetime of `self`.
        unsafe {
          let original = <$fn_type as Function>::from_ptr(self.trampoline_ptr());
          original($($nm),*)
        }
      }
    }

    $($doc)*
    impl<Ret: 'static, $($ty: 'static),*> $crate::StaticDetour<$fn_type> {
      /// Calls the original function, regardless of whether it is detoured or
      /// not.
      ///
      /// Available for all supported signatures, taking the same arguments
      /// as the target. It is `unsafe` if the target is.
      ///
      /// # Panics
      ///
      /// Panics if the detour has not been initialized.
      pub $($unsafety)? fn call(&self, $($nm : $ty),*) -> Ret {
        // SAFETY: The trampoline shares the target's signature, and remains
        // valid for the lifetime of `self` (i.e. forever).
        unsafe {
          let original = <$fn_type as Function>::from_ptr(self.trampoline_ptr());
          original($($nm),*)
        }
      }

      /// Creates the detour of `target`, redirected to `closure`.
      ///
      /// The detour is created disabled. It can only be initialized once;
      /// subsequent calls fail with [`Error::AlreadyInitialized`]. Returns
      /// `&self` to allow chaining, e.g. `initialize(..)?.enable()`.
      ///
      /// # Safety
      ///
      /// See [`TypedDetour::new`](crate::TypedDetour::new).
      ///
      /// [`Error::AlreadyInitialized`]: crate::Error::AlreadyInitialized
      pub unsafe fn initialize<Closure>(&self, target: $fn_type, closure: Closure) -> $crate::Result<&Self>
      where
        Closure: Fn($($ty),*) -> Ret + Send + Sync + 'static,
      {
        // SAFETY: Forwarded from the caller.
        unsafe { self.initialize_shared(target, ::alloc::boxed::Box::new(closure)) }
      }

      /// Replaces the detour closure, regardless of whether the detour is
      /// enabled or not.
      ///
      /// It may be called from within the detour itself. The previous closure
      /// is released once no thread is executing it.
      pub fn set_detour<Closure>(&self, closure: Closure)
      where
        Closure: Fn($($ty),*) -> Ret + Send + Sync + 'static,
      {
        self.set_detour_shared(::alloc::boxed::Box::new(closure));
      }
    }
  };

  ($($nm:ident : $ty:ident),*) => {
    impl_hookable!(@recurse ($($nm : $ty),*) ());
  };
}
