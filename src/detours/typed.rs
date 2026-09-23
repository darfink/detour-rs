use crate::detour::Detour;
use crate::error::Result;
use crate::{Function, HookableWith};
use core::marker::PhantomData;

/// A type-safe detour.
///
/// The same prototype is enforced for the target, the detour, and when
/// invoking the original function using [`call`](#method.call).
///
/// # Example
///
/// ```rust
/// # use detour::Result;
/// use detour::TypedDetour;
///
/// #[inline(never)]
/// fn add5(val: i32) -> i32 {
///   val + 5
/// }
///
/// #[inline(never)]
/// fn add10(val: i32) -> i32 {
///   val + 10
/// }
///
/// # fn main() -> Result<()> {
/// let mut hook = unsafe { TypedDetour::<fn(i32) -> i32>::new(add5, add10)? };
///
/// assert_eq!(add5(5), 10);
/// assert_eq!(hook.call(5), 10);
///
/// unsafe { hook.enable()? };
///
/// assert_eq!(add5(5), 15);
/// assert_eq!(hook.call(5), 10);
///
/// unsafe { hook.disable()? };
///
/// assert_eq!(add5(5), 10);
/// # Ok(())
/// # }
/// ```
pub struct TypedDetour<T: Function> {
  phantom: PhantomData<T>,
  detour: Detour,
}

impl<T: Function> TypedDetour<T> {
  /// Create a new hook given a target function and a compatible detour
  /// function.
  ///
  /// # Safety
  ///
  /// The target must be a function (e.g. not a Rust-ABI function that has
  /// been inlined into all of its callers) with the declared signature.
  pub unsafe fn new<D>(target: T, detour: D) -> Result<Self>
  where
    T: HookableWith<D>,
    D: Function,
  {
    // SAFETY: The signatures are compatible, as asserted by `HookableWith`.
    unsafe { Detour::new(target.to_ptr(), detour.to_ptr()) }.map(|detour| TypedDetour {
      phantom: PhantomData,
      detour,
    })
  }

  /// Enables the detour.
  ///
  /// # Safety
  ///
  /// The target must not be executing its prolog (i.e. the patched
  /// instructions) on another thread whilst the detour is being enabled.
  pub unsafe fn enable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.detour.enable() }
  }

  /// Disables the detour.
  ///
  /// # Safety
  ///
  /// See [`TypedDetour::enable`].
  pub unsafe fn disable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.detour.disable() }
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.detour.is_enabled()
  }

  /// Returns the trampoline, i.e. a function that invokes the original,
  /// undetoured target.
  ///
  /// Prefer [`call`](#method.call), unless the original function must be
  /// passed elsewhere (e.g. as a callback).
  ///
  /// # Safety
  ///
  /// The returned function must not be invoked after the detour is dropped.
  pub unsafe fn trampoline(&self) -> T {
    // SAFETY: The trampoline shares the target's signature.
    unsafe { T::from_ptr(self.trampoline_ptr()) }
  }

  /// Returns a pointer to the generated trampoline.
  pub(crate) fn trampoline_ptr(&self) -> *const () {
    self.detour.trampoline()
  }
}

impl<T: Function> core::fmt::Debug for TypedDetour<T> {
  fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    f.debug_tuple("TypedDetour").field(&self.detour).finish()
  }
}
