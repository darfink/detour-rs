#![cfg(all(not(windows), feature = "nightly"))]

use std::ffi::CString;
use std::os::raw::c_char;
use std::os::raw::c_int;
use retour::static_detour;

extern "C" {
  fn open(pathname: *const c_char, flags: c_int) -> c_int;
}

static_detour! {
    static Opentour: unsafe extern "C" fn(*const c_char, c_int) -> c_int;
}

fn definitely_open(_: *const c_char, _: c_int) -> c_int {
  let cstring = CString::new("/etc/timezone").unwrap();
  let fd = unsafe { Opentour.call(cstring.as_ptr() as *const c_char, 0) };
  assert!(fd > 0);
  fd
}

#[ctor::ctor]
fn main() {
    unsafe {
        Opentour.initialize(open, definitely_open).unwrap();
        Opentour.enable().unwrap();
    }
}
