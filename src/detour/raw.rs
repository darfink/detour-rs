use arch;
use error::*;

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

  /// Enables the detour.
  pub unsafe fn enable(&self) -> Result<()> {
    self.0.enable()
  }

  /// Disables the detour.
  pub unsafe fn disable(&self) -> Result<()> {
    self.0.disable()
  }

  /// Returns whether the detour is enabled or not.
  pub fn is_enabled(&self) -> bool {
    self.0.is_enabled()
  }

  /// Returns a reference to the generated trampoline.
  pub fn trampoline(&self) -> &() {
    self.0.trampoline()
  }
}
