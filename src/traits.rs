//! Traits describing detours and applicable functions.
//!
//! Several of the traits in this module are automatically implemented and
//! should generally not be implemented by users of this library.

/// Trait representing a function that can be used as a target or detour for
/// detouring.
///
/// It is implemented for function pointers with up to 14 arguments, for all
/// calling conventions supported by the target (e.g. `extern "C"`,
/// `extern "system"`, and `extern "thiscall"` on x86).
///
/// Function pointers with higher-ranked lifetimes (e.g. `fn(&str)`) cannot
/// implement this trait; use [`RawDetour`](crate::RawDetour) for those.
///
/// # Safety
///
/// Implementors must be function pointers, compatible with the pointer
/// returned by `to_ptr`.
pub unsafe trait Function: Sized + Copy + Sync + 'static {
  /// The argument types as a tuple.
  type Arguments;

  /// The return type.
  type Output;

  /// A closure type with a compatible signature (used by static detours).
  type Closure: ?Sized + Send + Sync;

  /// Constructs a `Function` from an untyped pointer.
  ///
  /// # Safety
  ///
  /// The pointer must point to a function with a compatible signature.
  unsafe fn from_ptr(ptr: *const ()) -> Self;

  /// Returns an untyped pointer for this function.
  fn to_ptr(&self) -> *const ();
}

/// Trait indicating that `Self` can be detoured by the given function `D`.
///
/// # Safety
///
/// `Self` and `D` must share the same signature and calling convention.
pub unsafe trait HookableWith<D: Function>: Function {}

// SAFETY: A function is always compatible with itself.
unsafe impl<T: Function> HookableWith<T> for T {}

impl_hookable! {
  __arg_0:  A, __arg_1:  B, __arg_2:  C, __arg_3:  D, __arg_4:  E, __arg_5:  F, __arg_6:  G,
  __arg_7:  H, __arg_8:  I, __arg_9:  J, __arg_10: K, __arg_11: L, __arg_12: M, __arg_13: N
}
