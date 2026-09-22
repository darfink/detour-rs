use crate::detour::Detour;
use crate::error::Result;

/// A raw detour.
///
/// # Example
///
/// ```rust
/// # use detour::Result;
/// use detour::RawDetour;
/// use std::mem;
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
/// let mut hook = unsafe { RawDetour::new(add5 as *const (), add10 as *const ())? };
///
/// assert_eq!(add5(5), 10);
/// assert_eq!(hook.is_enabled(), false);
///
/// unsafe { hook.enable()? };
/// assert!(hook.is_enabled());
///
/// let original: fn(i32) -> i32 = unsafe { mem::transmute(hook.trampoline()) };
///
/// assert_eq!(add5(5), 15);
/// assert_eq!(original(5), 10);
///
/// unsafe { hook.disable()? };
/// assert_eq!(add5(5), 10);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct RawDetour(Detour);

impl RawDetour {
  /// Constructs a new inline detour patcher.
  ///
  /// The hook is disabled by default. Even when this function is successful,
  /// there is no guarantee that the detour function will actually get called
  /// when the target function gets called. An invocation of the target
  /// function might for example get inlined in which case it is impossible to
  /// hook at runtime.
  ///
  /// # Safety
  ///
  /// `target` and `detour` must point to functions with compatible signatures
  /// and calling conventions.
  pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
    // SAFETY: Forwarded from the caller.
    unsafe { Detour::new(target, detour) }.map(RawDetour)
  }

  /// Enables the detour.
  ///
  /// # Safety
  ///
  /// The target must not be executing its prolog (i.e. the patched
  /// instructions) on another thread whilst the detour is being enabled.
  pub unsafe fn enable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.0.enable() }
  }

  /// Disables the detour.
  ///
  /// # Safety
  ///
  /// See [`RawDetour::enable`].
  pub unsafe fn disable(&self) -> Result<()> {
    // SAFETY: Forwarded from the caller.
    unsafe { self.0.disable() }
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.0.is_enabled()
  }

  /// Returns a pointer to the trampoline, which invokes the original target
  /// function regardless of whether the detour is enabled.
  ///
  /// The pointer is valid for as long as the detour exists.
  pub fn trampoline(&self) -> *const () {
    self.0.trampoline()
  }
}
