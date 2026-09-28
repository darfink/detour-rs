//! `static_detour!` & `signature!`, and internal macros.

/// Applies a `cfg` of the supported architectures to each item.
macro_rules! supported {
  ($($item:item)*) => {
    $(
      #[cfg(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))]
      $item
    )*
  };
}

/// Defines static, type-safe detours.
///
/// Each static is a handle to a detour of the declared function type. Since
/// the detour is a closure, it is created using `initialize` instead of a
/// constructor. See [`example::Example`](struct@crate::example::Example) for the
/// generated methods.
///
/// Unlike [`TypedDetour`](crate::TypedDetour), any function signature is
/// supported, including references (e.g. `fn(&str) -> usize`).
///
/// # Syntax
///
/// ```ignore
/// static_detour! {
///   [pub] static NAME_1: [for<'a, ..>] [unsafe] [extern "cc"] fn([argument]...) [-> ret];
///   [pub] static NAME_2: [for<'a, ..>] [unsafe] [extern "cc"] fn([argument]...) [-> ret];
///   ...
///   [pub] static NAME_N: [for<'a, ..>] [unsafe] [extern "cc"] fn([argument]...) [-> ret];
/// }
/// ```
///
/// Besides the static, a type with the same name is defined (which only
/// occupies the type namespace, so they do not collide). It is a zero-sized,
/// `Copy` handle to the detour, which is never released.
///
/// # Example
///
/// ```rust
/// use detour::static_detour;
///
/// static_detour! {
///   // The simplest detour
///   static Foo: fn();
///
///   // An unsafe public detour with a different calling convention
///   pub static PubFoo: unsafe extern "C" fn(i32) -> i32;
///
///   // A specific visibility modifier, and a trailing comma
///   pub(crate) static PubSelf: unsafe extern "C" fn(i32, i32,);
///
///   // References, with elided or explicit lifetimes
///   static Length: fn(&str) -> usize;
///   static Trim: for<'a> fn(&'a str) -> &'a str;
/// }
///
/// #[inline(never)]
/// fn length(text: &str) -> usize {
///   text.len()
/// }
///
/// # fn main() -> detour::Result<()> {
/// unsafe { Length.initialize(length, |text| Length.call(text) * 2)?.enable()? };
/// assert_eq!(length("abc"), 6);
/// assert_eq!(Length.call("abc"), 3);
/// # Ok(())
/// # }
/// ```
#[macro_export]
// Inspired by: https://github.com/Jascha-N/minhook-rs
macro_rules! static_detour {
  () => {};

  (@parsed [[$($attr:tt)*] $($names:tt)*] $($parts:tt)*) => {
    $crate::__cfg_attrs!([$crate::static_detour] [] [$($attr)*] [[$($attr)*] $($names)*] $($parts)*);
  };

  (@emit [$($cfg:tt)*] [[$($attr:tt)*] [$vis:vis] [$name:ident]] [$($lt:lifetime),*]
      [$($unsafety:tt)?] [$($abi:tt)*] [$output:ty] [$(($argument_name:ident : $argument:ty))*]) => {
    $($attr)*
    #[allow(non_camel_case_types)]
    #[derive(Clone, Copy)]
    $vis struct $name {
      _private: (),
    }

    $($attr)*
    #[allow(non_upper_case_globals)]
    $vis static $name: $name = $name { _private: () };

    $($cfg)*
    const _: () = {
      // SAFETY: The state is a single static.
      unsafe impl $crate::__private::StaticHandle for $name {
        type Function = for<$($lt),*> $($unsafety)? $($abi)* fn($($argument),*) -> $output;
        type Closure = dyn for<$($lt),*> Fn($($argument),*) -> $output + Send + Sync;

        #[inline]
        fn __state(self) -> &'static $crate::__private::StaticDetour<Self::Function, Self::Closure> {
          #[inline(never)]
          $($unsafety)? $($abi)* fn __ffi_detour<$($lt),*>($($argument_name: $argument),*) -> $output {
            <$name as $crate::__private::StaticHandle>::__state($name)
              .__with_detour(move |__detour| __detour($($argument_name),*))
          }

          // SAFETY: `Function` is a function pointer, and `__ffi_detour`
          // invokes the closure of this state.
          static STATE: $crate::__private::StaticDetour<
            <$name as $crate::__private::StaticHandle>::Function,
            <$name as $crate::__private::StaticHandle>::Closure,
          > = unsafe { $crate::__private::StaticDetour::__new(__ffi_detour) };
          &STATE
        }
      }

      impl $name {
        /// Creates the detour of `target`, redirected to `closure`.
        ///
        /// The detour is created disabled. It can only be initialized once;
        /// subsequent calls fail with
        /// [`Error::AlreadyInitialized`]($crate::Error::AlreadyInitialized).
        /// Returns `self` to allow chaining, e.g. `initialize(..)?.enable()`.
        ///
        /// # Safety
        ///
        /// See [`TypedDetour::new`]($crate::TypedDetour::new).
        pub unsafe fn initialize(
          self,
          target: for<$($lt),*> $($unsafety)? $($abi)* fn($($argument),*) -> $output,
          closure: impl for<$($lt),*> Fn($($argument),*) -> $output + Send + Sync + 'static,
        ) -> $crate::Result<Self> {
          // SAFETY: Forwarded from the caller.
          unsafe {
            <Self as $crate::__private::StaticHandle>::__state(self)
              .__initialize(target, $crate::__private::Box::new(closure))?
          };
          ::core::result::Result::Ok(self)
        }

        /// Calls the original function, regardless of whether it is
        /// detoured or not. It is `unsafe` if the target is.
        ///
        /// # Panics
        ///
        /// Panics if the detour has not been initialized.
        #[track_caller]
        #[allow(unused_unsafe)]
        pub $($unsafety)? fn call<$($lt),*>(self, $($argument_name: $argument),*) -> $output {
          // SAFETY: The caller upholds the target's contract (if `unsafe`).
          unsafe {
            (<Self as $crate::__private::StaticHandle>::__state(self).__original())($($argument_name),*)
          }
        }

        /// Replaces the detour closure, regardless of whether the detour is
        /// enabled or not.
        ///
        /// It may be called from within the detour itself. The previous
        /// closure is released once no thread is executing it.
        pub fn set_detour(
          self,
          closure: impl for<$($lt),*> Fn($($argument),*) -> $output + Send + Sync + 'static,
        ) {
          <Self as $crate::__private::StaticHandle>::__state(self)
            .__set_detour($crate::__private::Box::new(closure));
        }

        /// Enables the detour.
        ///
        /// # Safety
        ///
        /// See [`TypedDetour::enable`]($crate::TypedDetour::enable).
        pub unsafe fn enable(self) -> $crate::Result<()> {
          // SAFETY: Forwarded from the caller.
          unsafe { <Self as $crate::__private::StaticHandle>::__state(self).__enable() }
        }

        /// Disables the detour.
        ///
        /// # Safety
        ///
        /// See [`TypedDetour::enable`]($crate::TypedDetour::enable).
        pub unsafe fn disable(self) -> $crate::Result<()> {
          // SAFETY: Forwarded from the caller.
          unsafe { <Self as $crate::__private::StaticHandle>::__state(self).__disable() }
        }

        /// Returns whether the detour is enabled or not.
        pub fn is_enabled(self) -> bool {
          <Self as $crate::__private::StaticHandle>::__state(self).__is_enabled()
        }

        /// Returns the trampoline, i.e. a function that invokes the
        /// original, undetoured target, or `None` if the detour is not
        /// initialized.
        ///
        /// Prefer `call`, unless the original function must be passed
        /// elsewhere (e.g. as a callback, or to foreign code). Since static
        /// detours are never released, the trampoline remains valid forever.
        pub fn trampoline(self) -> ::core::option::Option<
          for<$($lt),*> $($unsafety)? $($abi)* fn($($argument),*) -> $output
        > {
          <Self as $crate::__private::StaticHandle>::__state(self).__trampoline()
        }
      }

      impl ::core::fmt::Debug for $name {
        fn fmt(&self, __f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
          ::core::fmt::Debug::fmt(<Self as $crate::__private::StaticHandle>::__state(*self), __f)
        }
      }
    };
  };

  (@split $extra:tt [$($signature:tt)*] ; $($rest:tt)*) => {
    $crate::__signature!([$crate::static_detour] $extra $($signature)*);
    $crate::static_detour!($($rest)*);
  };
  (@split $extra:tt [$($signature:tt)*] $next:tt $($rest:tt)*) => {
    $crate::static_detour!(@split $extra [$($signature)* $next] $($rest)*);
  };

  ($(#[$($attr:tt)*])* $vis:vis static $name:ident : $($rest:tt)*) => {
    $crate::static_detour!(@split [[$(#[$($attr)*])*] [$vis] [$name]] [] $($rest)*);
  };
}

/// Defines a type implementing [`Function`](crate::Function) for a signature.
///
/// [`Function`](crate::Function) is implemented for function pointers, except
/// those with higher-ranked lifetimes, such as `fn(&str) -> usize`. For those,
/// this macro defines a transparent newtype, which can be used with
/// [`TypedDetour`](crate::TypedDetour). The syntax is that of a tuple struct
/// with a single function pointer field.
///
/// Besides `Clone`, `Copy` and `Debug`, the type has a `call` method, which
/// invokes the function (it is `unsafe` if the function is). The original
/// function of a detour is called with
/// [`TypedDetour::original`](crate::TypedDetour::original), since
/// [`TypedDetour::call`](crate::TypedDetour::call) is not available for such
/// types.
///
/// Conditional attributes (`cfg` and `cfg_attr`) also apply to the generated
/// implementations.
///
/// For static detours, this is not needed: [`static_detour!`] supports any
/// signature.
///
/// # Example
///
/// ```rust
/// use detour::{TypedDetour, signature};
///
/// signature! {
///   /// The signature of `length`.
///   struct Length(fn(&str) -> usize);
///
///   pub struct Trim(pub for<'a> unsafe extern "C" fn(&'a u8) -> &'a u8);
/// }
///
/// #[inline(never)]
/// fn length(text: &str) -> usize {
///   text.len()
/// }
///
/// fn double_length(text: &str) -> usize {
///   text.len() * 2
/// }
///
/// # fn main() -> detour::Result<()> {
/// let detour = unsafe { TypedDetour::new(Length(length), Length(double_length))? };
/// unsafe { detour.enable()? };
///
/// assert_eq!(length("abc"), 6);
/// assert_eq!(detour.original().call("abc"), 3);
/// # Ok(())
/// # }
/// ```
///
/// [`static_detour!`]: crate::static_detour
#[macro_export]
macro_rules! signature {
  () => {};

  (@parsed [[$($attr:tt)*] $($names:tt)*] $($parts:tt)*) => {
    $crate::__cfg_attrs!([$crate::signature] [] [$($attr)*] [[$($attr)*] $($names)*] $($parts)*);
  };

  (@emit [$($cfg:tt)*] [[$($attr:tt)*] [$vis:vis] [$name:ident] [$($field_vis:tt)*]]
      [$($lt:lifetime),*] [$($unsafety:tt)?] [$($abi:tt)*] [$output:ty]
      [$(($argument_name:ident : $argument:ty))*]) => {
    $($attr)*
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug)]
    $vis struct $name(
      $($field_vis)* for<$($lt),*> $($unsafety)? $($abi)* fn($($argument),*) -> $output
    );

    $($cfg)*
    const _: () = {
      impl $crate::__private::Sealed for $name {}

      // SAFETY: The type is a transparent function pointer.
      unsafe impl $crate::Function for $name {
        type Original<'__detour> = __DetourOriginal<'__detour>;

        unsafe fn from_ptr(__ptr: *const ()) -> Self {
          // SAFETY: Function pointers and data pointers share representation
          // on all supported platforms; validity is guaranteed by the caller.
          $name(unsafe {
            ::core::mem::transmute::<
              *const (),
              for<$($lt),*> $($unsafety)? $($abi)* fn($($argument),*) -> $output,
            >(__ptr)
          })
        }

        fn to_ptr(&self) -> *const () {
          self.0 as *const ()
        }

        unsafe fn __original<'__detour>(self) -> __DetourOriginal<'__detour> {
          // SAFETY: Forwarded from the caller.
          __DetourOriginal(unsafe { $crate::Original::__new(self) })
        }
      }

      /// The original function of a detour (see
      /// [`TypedDetour::original`]($crate::TypedDetour::original)).
      #[derive(Clone, Copy, Debug)]
      pub struct __DetourOriginal<'__detour>($crate::Original<'__detour, $name>);

      impl __DetourOriginal<'_> {
        /// Calls the original function. It is `unsafe` if the function is.
        #[allow(unused_unsafe)]
        pub $($unsafety)? fn call<$($lt),*>(self, $($argument_name: $argument),*) -> $output {
          // SAFETY: The function is only used within its lifetime, and the
          // caller upholds its contract (if `unsafe`).
          unsafe { (self.0.__function().0)($($argument_name),*) }
        }
      }

      impl $name {
        /// Calls the function. It is `unsafe` if the function is.
        #[allow(unused_unsafe)]
        pub $($unsafety)? fn call<$($lt),*>(self, $($argument_name: $argument),*) -> $output {
          // SAFETY: The caller upholds the function's contract (if `unsafe`).
          unsafe { (self.0)($($argument_name),*) }
        }
      }
    };
  };

  ($(#[$($attr:tt)*])* $vis:vis struct $name:ident (pub($($restriction:tt)*) $($signature:tt)*);
      $($rest:tt)*) => {
    $crate::__signature!([$crate::signature]
      [[$(#[$($attr)*])*] [$vis] [$name] [pub($($restriction)*)]] $($signature)*);
    $crate::signature!($($rest)*);
  };
  ($(#[$($attr:tt)*])* $vis:vis struct $name:ident (pub $($signature:tt)*); $($rest:tt)*) => {
    $crate::__signature!([$crate::signature] [[$(#[$($attr)*])*] [$vis] [$name] [pub]] $($signature)*);
    $crate::signature!($($rest)*);
  };
  ($(#[$($attr:tt)*])* $vis:vis struct $name:ident ($($signature:tt)*); $($rest:tt)*) => {
    $crate::__signature!([$crate::signature] [[$(#[$($attr)*])*] [$vis] [$name] []] $($signature)*);
    $crate::signature!($($rest)*);
  };
}

/// Keeps the conditional attributes (`cfg`, and the `cfg` parts of
/// `cfg_attr`, including nested ones) of an item, and passes them to a macro, so its implementations
/// are conditional as well:
///
/// `$callback! { @emit [attributes] $args... }`
#[doc(hidden)]
#[macro_export]
macro_rules! __cfg_attrs {
  // The `cfg` attributes within a `cfg_attr`
  (@inner $cb:tt $kept:tt $pred:tt [] [] $rest:tt $($args:tt)*) => {
    $crate::__cfg_attrs!($cb $kept $rest $($args)*);
  };
  (@inner $cb:tt [$($kept:tt)*] [$pred:meta] [$($inner:tt)+] [] $rest:tt $($args:tt)*) => {
    $crate::__cfg_attrs!($cb [$($kept)* #[cfg_attr($pred, $($inner)+)]] $rest $($args)*);
  };
  (@inner $cb:tt $kept:tt $pred:tt [$($inner:tt)*] [cfg $condition:tt $(, $($more:tt)*)?]
      $rest:tt $($args:tt)*) => {
    $crate::__cfg_attrs!(@inner $cb $kept $pred [$($inner)* cfg $condition,] [$($($more)*)?]
      $rest $($args)*);
  };
  // A nested `cfg_attr(q, ..)` within `cfg_attr(p, ..)` is equivalent to
  // `cfg_attr(all(p, q), ..)`, which is filtered as any other attribute
  (@inner $cb:tt $kept:tt [$pred:meta] $inner:tt
      [cfg_attr($nested:meta, $($nested_inner:tt)*) $(, $($more:tt)*)?]
      [$($rest:tt)*] $($args:tt)*) => {
    $crate::__cfg_attrs!(@inner $cb $kept [$pred] $inner [$($($more)*)?]
      [#[cfg_attr(all($pred, $nested), $($nested_inner)*)] $($rest)*] $($args)*);
  };
  (@inner $cb:tt $kept:tt $pred:tt $inner:tt [$other:meta $(, $($more:tt)*)?]
      $rest:tt $($args:tt)*) => {
    $crate::__cfg_attrs!(@inner $cb $kept $pred $inner [$($($more)*)?] $rest $($args)*);
  };

  ([$($cb:tt)*] [$($kept:tt)*] [] $($args:tt)*) => {
    $($cb)*! { @emit [$($kept)*] $($args)* }
  };
  ($cb:tt [$($kept:tt)*] [#[cfg $condition:tt] $($rest:tt)*] $($args:tt)*) => {
    $crate::__cfg_attrs!($cb [$($kept)* #[cfg $condition]] [$($rest)*] $($args)*);
  };
  ($cb:tt $kept:tt [#[cfg_attr($pred:meta, $($inner:tt)*)] $($rest:tt)*] $($args:tt)*) => {
    $crate::__cfg_attrs!(@inner $cb $kept [$pred] [] [$($inner)*] [$($rest)*] $($args)*);
  };
  ($cb:tt $kept:tt [#[$($other:tt)*] $($rest:tt)*] $($args:tt)*) => {
    $crate::__cfg_attrs!($cb $kept [$($rest)*] $($args)*);
  };
}

/// Parses a function pointer type, and passes its parts to a macro:
///
/// `$callback! { @parsed $extra [lifetimes] [unsafe] [abi] [output] [(name: type)...] }`
#[doc(hidden)]
#[macro_export]
macro_rules! __signature {
  (@qualifiers $cb:tt $extra:tt $lts:tt unsafe extern $abi:literal fn $($rest:tt)*) => {
    $crate::__signature!(@arguments $cb $extra $lts [unsafe] [extern $abi] $($rest)*);
  };
  (@qualifiers $cb:tt $extra:tt $lts:tt unsafe extern fn $($rest:tt)*) => {
    $crate::__signature!(@arguments $cb $extra $lts [unsafe] [extern "C"] $($rest)*);
  };
  (@qualifiers $cb:tt $extra:tt $lts:tt unsafe fn $($rest:tt)*) => {
    $crate::__signature!(@arguments $cb $extra $lts [unsafe] [] $($rest)*);
  };
  (@qualifiers $cb:tt $extra:tt $lts:tt extern $abi:literal fn $($rest:tt)*) => {
    $crate::__signature!(@arguments $cb $extra $lts [] [extern $abi] $($rest)*);
  };
  (@qualifiers $cb:tt $extra:tt $lts:tt extern fn $($rest:tt)*) => {
    $crate::__signature!(@arguments $cb $extra $lts [] [extern "C"] $($rest)*);
  };
  (@qualifiers $cb:tt $extra:tt $lts:tt fn $($rest:tt)*) => {
    $crate::__signature!(@arguments $cb $extra $lts [] [] $($rest)*);
  };

  (@qualifiers $($invalid:tt)*) => {
    ::core::compile_error!("expected a function pointer type, e.g. `unsafe extern \"C\" fn(i32)`");
  };

  (@arguments $cb:tt $extra:tt $lts:tt $unsafety:tt $abi:tt
      ($($argument:ty),* $(,)?) -> $output:ty) => {
    $crate::__signature!(@names $cb $extra $lts $unsafety $abi [$output] [] [$($argument),*]
      [__arg_0 __arg_1 __arg_2 __arg_3 __arg_4 __arg_5 __arg_6
       __arg_7 __arg_8 __arg_9 __arg_10 __arg_11 __arg_12 __arg_13]);
  };
  (@arguments $cb:tt $extra:tt $lts:tt $unsafety:tt $abi:tt ($($argument:ty),* $(,)?)) => {
    $crate::__signature!(@names $cb $extra $lts $unsafety $abi [()] [] [$($argument),*]
      [__arg_0 __arg_1 __arg_2 __arg_3 __arg_4 __arg_5 __arg_6
       __arg_7 __arg_8 __arg_9 __arg_10 __arg_11 __arg_12 __arg_13]);
  };

  (@arguments $($invalid:tt)*) => {
    ::core::compile_error!("expected function arguments & an optional return type, e.g. `(i32) -> i32`");
  };

  // Associate each argument type with a name
  (@names [$($cb:tt)*] $extra:tt $lts:tt $unsafety:tt $abi:tt $output:tt $named:tt [] $unused:tt) => {
    $($cb)*! { @parsed $extra $lts $unsafety $abi $output $named }
  };
  (@names $cb:tt $extra:tt $lts:tt $unsafety:tt $abi:tt $output:tt
      [$($named:tt)*] [$argument:ty $(, $arguments:ty)*] [$next:ident $($names:ident)*]) => {
    $crate::__signature!(@names $cb $extra $lts $unsafety $abi $output
      [$($named)* ($next: $argument)] [$($arguments),*] [$($names)*]);
  };

  ($cb:tt $extra:tt for<$($lt:lifetime),* $(,)?> $($rest:tt)*) => {
    $crate::__signature!(@qualifiers $cb $extra [$($lt),*] $($rest)*);
  };
  ($cb:tt $extra:tt $($rest:tt)*) => {
    $crate::__signature!(@qualifiers $cb $extra [] $($rest)*);
  };
}

/// Implements `Function`, `HookableWith`, and the signature-specific methods
/// of `TypedDetour`, for all supported function pointers.
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
    impl<Ret: 'static, $($ty: 'static),*> $crate::traits::private::Sealed for $fn_type {}

    // SAFETY: Implemented for function pointers only.
    unsafe impl<Ret: 'static, $($ty: 'static),*> Function for $fn_type {
      type Original<'a> = $crate::Original<'a, Self>;

      unsafe fn from_ptr(ptr: *const ()) -> Self {
        // SAFETY: Function pointers and data pointers share representation on
        // all supported platforms; validity is guaranteed by the caller.
        unsafe { ::core::mem::transmute::<*const (), Self>(ptr) }
      }

      fn to_ptr(&self) -> *const () {
        *self as *const ()
      }

      unsafe fn __original<'a>(self) -> Self::Original<'a> {
        // SAFETY: Forwarded from the caller.
        unsafe { $crate::Original::__new(self) }
      }
    }

    $($doc)*
    impl<Ret: 'static, $($ty: 'static),*> $crate::Original<'_, $fn_type> {
      /// Calls the original function.
      ///
      /// Available for all supported signatures, taking the same arguments
      /// as the target. It is `unsafe` if the target is.
      #[allow(unused_unsafe)]
      pub $($unsafety)? fn call(self, $($nm : $ty),*) -> Ret {
        // SAFETY: The trampoline is borrowed from its detour, and the caller
        // upholds the target's contract (if `unsafe`).
        unsafe { (self.__function())($($nm),*) }
      }
    }

    $($doc)*
    impl<Ret: 'static, $($ty: 'static),*> $crate::TypedDetour<$fn_type> {
      /// Calls the original function, regardless of whether it is detoured or
      /// not.
      ///
      /// Shorthand for [`original().call(..)`](Self::original). Available
      /// for all supported signatures, taking the same arguments as the
      /// target. It is `unsafe` if the target is.
      #[allow(unused_unsafe)]
      pub $($unsafety)? fn call(&self, $($nm : $ty),*) -> Ret {
        // SAFETY: Forwarded from the caller (if `unsafe`).
        unsafe { self.original().call($($nm),*) }
      }
    }

  };

  ($($nm:ident : $ty:ident),*) => {
    impl_hookable!(@recurse ($($nm : $ty),*) ());
  };
}
