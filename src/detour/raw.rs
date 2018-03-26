use arch;
use error::*;
use std::ops::{Deref, DerefMut};

/// A type-less wrapper around [Detour](./struct.Detour.html).
///
/// # Example
///
/// ```rust
/// use detour::RawDetour;
/// use std::mem;
///
/// fn add5(val: i32) -> i32 {
///   val + 5
/// }
/// fn add10(val: i32) -> i32 {
///   val + 10
/// }
///
/// let mut hook = unsafe { RawDetour::new(add5 as *const (), add10 as *const ()).unwrap() };
///
/// assert_eq!(add5(5), 10);
/// assert_eq!(hook.is_enabled(), false);
///
/// unsafe {
///   hook.enable().unwrap();
///   assert!(hook.is_enabled());
///
///   let original: fn(i32) -> i32 = mem::transmute(hook.trampoline());
///
///   assert_eq!(add5(5), 15);
///   assert_eq!(original(5), 10);
///
///   hook.disable().unwrap();
/// }
/// assert_eq!(add5(5), 10);
/// ```
#[derive(Debug)]
pub struct RawDetour(arch::Detour);

// TODO: stop all threads in target during patch?
impl RawDetour {
  /// Constructs a new inline detour patcher.
  ///
  /// The hook is disabled by default. Even when this function is succesful,
  /// there is no guaranteee that the detour function will actually get called
  /// when the target function gets called. An invocation of the target
  /// function might for example get inlined in which case it is impossible to
  /// hook at runtime.
  pub unsafe fn new(target: *const (), detour: *const ()) -> Result<Self> {
    arch::Detour::new(target, detour).map(RawDetour)
  }
}

impl Deref for RawDetour {
  type Target = arch::Detour;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for RawDetour {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}
