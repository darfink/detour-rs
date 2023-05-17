use retour::Result;
use std::mem;

type FnAdd = extern "C" fn(i32, i32) -> i32;

#[inline(never)]
extern "C" fn sub_detour(x: i32, y: i32) -> i32 {
  unsafe { std::ptr::read_volatile(&x as *const i32) - y }
}

mod raw {
  use super::*;
  use retour::RawDetour;

  #[test]
  fn test() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      unsafe { std::ptr::read_volatile(&x as *const i32) + y }
    }

    unsafe {
      let hook = RawDetour::new(add as *const (), sub_detour as *const ())
        .expect("target or source is not usable for detouring");

      assert_eq!(add(10, 5), 15);
      assert!(!hook.is_enabled());

      hook.enable()?;
      {
        assert!(hook.is_enabled());

        // The `add` function is hooked, but can be called using the trampoline
        let trampoline: FnAdd = mem::transmute(hook.trampoline());

        // Call the original function
        assert_eq!(trampoline(10, 5), 15);

        // Call the hooked function (i.e `add → sub_detour`)
        assert_eq!(add(10, 5), 5);
      }
      hook.disable()?;

      // With the hook disabled, the function is restored
      assert!(!hook.is_enabled());
      assert_eq!(add(10, 5), 15);
    }
    Ok(())
  }
}

mod generic {
  use super::*;
  use retour::GenericDetour;

  #[test]
  fn test() -> Result<()> {
    #[inline(never)]
    extern "C" fn add(x: i32, y: i32) -> i32 {
      unsafe { std::ptr::read_volatile(&x as *const i32) + y }
    }

    unsafe {
      let hook = GenericDetour::<FnAdd>::new(add, sub_detour)
        .expect("target or source is not usable for detouring");

      assert_eq!(add(10, 5), 15);
      assert_eq!(hook.call(10, 5), 15);
      hook.enable()?;
      {
        assert_eq!(hook.call(10, 5), 15);
        assert_eq!(add(10, 5), 5);
      }
      hook.disable()?;
      assert_eq!(hook.call(10, 5), 15);
      assert_eq!(add(10, 5), 15);
    }
    Ok(())
  }
}

#[cfg(feature = "nightly")]
mod statik {
  use super::*;
  use retour::static_detour;

  #[inline(never)]
  unsafe extern "C" fn add(x: i32, y: i32) -> i32 {
    std::ptr::read_volatile(&x as *const i32) + y
  }

  static_detour! {
    #[doc="Test with attributes"]
    pub static DetourAdd: unsafe extern "C" fn(i32, i32) -> i32;
  }

  #[test]
  fn test() -> Result<()> {
    unsafe {
      DetourAdd.initialize(add, |x, y| x - y)?;

      assert_eq!(add(10, 5), 15);
      assert_eq!(DetourAdd.is_enabled(), false);

      DetourAdd.enable()?;
      {
        assert!(DetourAdd.is_enabled());
        assert_eq!(DetourAdd.call(10, 5), 15);
        assert_eq!(add(10, 5), 5);
      }
      DetourAdd.disable()?;

      assert_eq!(DetourAdd.is_enabled(), false);
      assert_eq!(DetourAdd.call(10, 5), 15);
      assert_eq!(add(10, 5), 15);
    }
    Ok(())
  }
}
