/// A macro for defining static, type-safe detours.
///
/// This macro defines one or more [StaticDetour](./struct.StaticDetour.html)s.
///
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
///   // A specific visibility modifier
///   pub(crate) static PubSelf: unsafe extern "C" fn();
/// }
/// # fn main() { }
/// ```
#[cfg(feature = "nightly")]
#[macro_export]
// Inspired by: https://github.com/Jascha-N/minhook-rs
macro_rules! static_detour {
  // 1 — meta attributes
  (@parse_attributes ($($input:tt)*) | #[$attribute:meta] $($rest:tt)*) => {
    static_detour!(@parse_attributes ($($input)* $attribute) | $($rest)*);
  };
  (@parse_attributes ($($input:tt)*) | $($rest:tt)+) => {
    static_detour!(@parse_access_modifier (($($input)*)) | $($rest)*);
  };

  // 2 — pub modifier (path/scope/yes/no)
  (@parse_access_modifier ($($input:tt)*) | pub(in $vis:path) static $($rest:tt)*) => {
    static_detour!(@parse_name ($($input)* (pub(in $vis))) | $($rest)*);
  };
  (@parse_access_modifier ($($input:tt)*) | pub($vis:tt) static $($rest:tt)*) => {
    static_detour!(@parse_name ($($input)* (pub($vis))) | $($rest)*);
  };
  (@parse_access_modifier ($($input:tt)*) | pub static $($rest:tt)*) => {
    static_detour!(@parse_name ($($input)* (pub)) | $($rest)*);
  };
  (@parse_access_modifier ($($input:tt)*) | static $($rest:tt)*) => {
    static_detour!(@parse_name ($($input)* ()) | $($rest)*);
  };

  // 3 — detour name
  (@parse_name ($($input:tt)*) | $name:ident : $($rest:tt)*) => {
    static_detour!(@parse_unsafe ($($input)* ($name)) | $($rest)*);
  };

  // 4 — unsafe modifier (yes/no)
  (@parse_unsafe ($($input:tt)*) | unsafe $($rest:tt)*) => {
    static_detour!(@parse_calling_convention ($($input)*) (unsafe) | $($rest)*);
  };
  (@parse_unsafe ($($input:tt)*) | $($rest:tt)*) => {
    static_detour!(@parse_calling_convention ($($input)*) () | $($rest)*);
  };

  // 5 — calling convention (extern "XXX"/extern/-)
  (@parse_calling_convention
      ($($input:tt)*) ($($modifier:tt)*) | extern $cc:tt fn $($rest:tt)*) => {
    static_detour!(@parse_prototype ($($input)* ($($modifier)* extern $cc)) | $($rest)*);
  };
  (@parse_calling_convention
      ($($input:tt)*) ($($modifier:tt)*) | extern fn $($rest:tt)*) => {
    static_detour!(@parse_prototype ($($input)* ($($modifier)* extern)) | $($rest)*);
  };
  (@parse_calling_convention ($($input:tt)*) ($($modifier:tt)*) | fn $($rest:tt)*) => {
    static_detour!(@parse_prototype ($($input)* ($($modifier)*)) | $($rest)*);
  };

  // 6 — argument and return type (return/void)
  (@parse_prototype
      ($($input:tt)*) | ($($argument_type:ty),*) -> $return_type:ty ; $($rest:tt)*) => {
    static_detour!(
      @parse_terminator ($($input)* ($($argument_type)*) ($return_type)) | ; $($rest)*);
  };
  (@parse_prototype ($($input:tt)*) | ($($argument_type:ty),*) $($rest:tt)*) => {
    static_detour!(@parse_terminator ($($input)* ($($argument_type)*) (())) | $($rest)*);
  };

  // 7 — semicolon terminator
  (@parse_terminator ($($input:tt)*) | ; $($rest:tt)*) => {
    static_detour!(@parse_entries ($($input)*) | $($rest)*);
  };

  // 8 - additional detours (multiple/single)
  (@parse_entries ($($input:tt)*) | $($rest:tt)+) => {
    static_detour!(@aggregate $($input)*);
    static_detour!($($rest)*);
  };
  (@parse_entries ($($input:tt)*) | ) => {
    static_detour!(@aggregate $($input)*);
  };

  // 9 - aggregate data for the generate function
  (@aggregate ($($attribute:meta)*) ($($visibility:tt)*) ($name:ident)
              ($($modifier:tt)*) ($($argument_type:ty)*) ($return_type:ty)) => {
    static_detour!(@argument_names (create_detour)(
      ($($attribute)*) ($($visibility)*) ($name)
      ($($modifier)*) ($($argument_type)*) ($return_type)
      ($($modifier)* fn ($($argument_type),*) -> $return_type)
    )($($argument_type)*));
  };

  // 10 - detour type implementation
  (@create_detour ($($argument_name:ident)*) ($($attribute:meta)*) ($($visibility:tt)*)
                  ($name:ident) ($($modifier:tt)*) ($($argument_type:ty)*)
                  ($return_type:ty) ($fn_type:ty)) => {
    static_detour!(@generate
      #[allow(non_upper_case_globals)]
      $(#[$attribute])*
      $($visibility)* static $name: $crate::StaticDetour<$fn_type> = {
        #[inline(never)]
        #[allow(unused_unsafe)]
        $($modifier) * fn __ffi_detour(
            $($argument_name: $argument_type),*) -> $return_type {
          #[allow(unused_unsafe)]
          ($name.__detour())($($argument_name),*)
        }

        $crate::StaticDetour::__new(__ffi_detour)
      };
    );
  };

  // Associates each argument type with a dummy name.
  (@argument_names ($label:ident) ($($input:tt)*) ($($token:tt)*)) => {
    static_detour!(@argument_names ($label) ($($input)*)(
      __arg_0  __arg_1  __arg_2  __arg_3  __arg_4  __arg_5  __arg_6
      __arg_7  __arg_8  __arg_9  __arg_10 __arg_11 __arg_12 __arg_13
    )($($token)*)());
  };
  (@argument_names
      ($label:ident)
      ($($input:tt)*)
      ($hd_name:tt $($tl_name:tt)*)
      ($hd:tt $($tl:tt)*) ($($acc:tt)*)) => {
    static_detour!(
      @argument_names ($label) ($($input)*) ($($tl_name)*) ($($tl)*) ($($acc)* $hd_name));
  };
  (@argument_names ($label:ident) ($($input:tt)*) ($($name:tt)*) () ($($acc:tt)*)) => {
    static_detour!(@$label ($($acc)*) $($input)*);
  };

  (@generate $item:item) => { $item };

  // Bootstrapper
  ($($t:tt)+) => {
    static_detour!(@parse_attributes () | $($t)+);
  };
}

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

  (@impl_all ($($nm:ident : $ty:ident),*)) => {
    impl_hookable!(@impl_pair ($($nm : $ty),*) (                  fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "cdecl"    fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "stdcall"  fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "fastcall" fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "win64"    fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "C"        fn($($ty),*) -> Ret));
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "system"   fn($($ty),*) -> Ret));

    #[cfg(feature = "nightly")]
    impl_hookable!(@impl_pair ($($nm : $ty),*) (extern "thiscall" fn($($ty),*) -> Ret));
  };

  (@impl_pair ($($nm:ident : $ty:ident),*) ($($fn_t:tt)*)) => {
    impl_hookable!(@impl_fun ($($nm : $ty),*) ($($fn_t)*) (unsafe $($fn_t)*));
  };

  (@impl_fun ($($nm:ident : $ty:ident),*) ($safe_type:ty) ($unsafe_type:ty)) => {
    impl_hookable!(@impl_core ($($nm : $ty),*) ($safe_type));
    impl_hookable!(@impl_core ($($nm : $ty),*) ($unsafe_type));

    impl_hookable!(@impl_unsafe ($($nm : $ty),*) ($unsafe_type) ($safe_type));
    impl_hookable!(@impl_safe ($($nm : $ty),*) ($safe_type));
  };

  (@impl_unsafe ($($nm:ident : $ty:ident),*) ($target:ty) ($detour:ty)) => {
    unsafe impl<Ret: 'static, $($ty: 'static),*> HookableWith<$detour> for $target {}

    #[cfg(feature = "nightly")]
    impl<Ret: 'static, $($ty: 'static),*> $crate::StaticDetour<$target> {
      #[doc(hidden)]
      pub unsafe fn call(&self, $($nm : $ty),*) -> Ret {
        let original: $target = ::std::mem::transmute(self.trampoline().expect("calling detour trampoline"));
        original($($nm),*)
      }
    }

    impl<Ret: 'static, $($ty: 'static),*> $crate::GenericDetour<$target> {
      #[doc(hidden)]
      pub unsafe fn call(&self, $($nm : $ty),*) -> Ret {
        let original: $target = ::std::mem::transmute(self.trampoline());
        original($($nm),*)
      }
    }
  };

  (@impl_safe ($($nm:ident : $ty:ident),*) ($fn_type:ty)) => {
    #[cfg(feature = "nightly")]
    impl<Ret: 'static, $($ty: 'static),*> $crate::StaticDetour<$fn_type> {
      #[doc(hidden)]
      pub fn call(&self, $($nm : $ty),*) -> Ret {
        unsafe {
          let original: $fn_type = ::std::mem::transmute(self.trampoline().expect("calling detour trampoline"));
          original($($nm),*)
        }
      }
    }

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
