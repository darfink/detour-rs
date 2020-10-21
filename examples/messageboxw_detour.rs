#![cfg(all(windows, feature = "nightly"))]
//! A `MessageBoxW` detour example.
//!
//! Ensure the crate is compiled as a 'cdylib' library to allow C interop.
use detour::static_detour;
use std::error::Error;
use std::{ffi::CString, iter, mem};
use winapi::ctypes::c_int;
use winapi::shared::minwindef::{BOOL, DWORD, HINSTANCE, LPVOID, TRUE, UINT};
use winapi::shared::windef::HWND;
use winapi::um::libloaderapi::{GetModuleHandleW, GetProcAddress};
use winapi::um::winnt::{DLL_PROCESS_ATTACH, LPCWSTR};

static_detour! {
  static MessageBoxWHook: unsafe extern "system" fn(HWND, LPCWSTR, LPCWSTR, UINT) -> c_int;
}

// A type alias for `MessageBoxW` (makes the transmute easy on the eyes)
type FnMessageBoxW = unsafe extern "system" fn(HWND, LPCWSTR, LPCWSTR, UINT) -> c_int;

/// Called when the DLL is attached to the process.
unsafe fn main() -> Result<(), Box<dyn Error>> {
  // Retrieve an absolute address of `MessageBoxW`. This is required for
  // libraries due to the import address table. If `MessageBoxW` would be
  // provided directly as the target, it would only hook this DLL's
  // `MessageBoxW`. Using the method below an absolute address is retrieved
  // instead, detouring all invocations of `MessageBoxW` in the active process.
  let address = get_module_symbol_address("user32.dll", "MessageBoxW")
    .expect("could not find 'MessageBoxW' address");
  let target: FnMessageBoxW = mem::transmute(address);

  // Initialize AND enable the detour (the 2nd parameter can also be a closure)
  MessageBoxWHook
    .initialize(target, messageboxw_detour)?
    .enable()?;
  Ok(())
}

/// Called whenever `MessageBoxW` is invoked in the process.
fn messageboxw_detour(hwnd: HWND, text: LPCWSTR, _caption: LPCWSTR, u_type: UINT) -> c_int {
  // Call the original `MessageBoxW`, but replace the caption
  let replaced_caption = "Detoured!\0".encode_utf16().collect::<Vec<u16>>();
  unsafe { MessageBoxWHook.call(hwnd, text, replaced_caption.as_ptr() as _, u_type) }
}

/// Returns a module symbol's absolute address.
fn get_module_symbol_address(module: &str, symbol: &str) -> Option<usize> {
  let module = module
    .encode_utf16()
    .chain(iter::once(0))
    .collect::<Vec<u16>>();
  let symbol = CString::new(symbol).unwrap();
  unsafe {
    let handle = GetModuleHandleW(module.as_ptr());
    match GetProcAddress(handle, symbol.as_ptr()) as usize {
      0 => None,
      n => Some(n),
    }
  }
}

#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllMain(
  _module: HINSTANCE,
  call_reason: DWORD,
  _reserved: LPVOID,
) -> BOOL {
  if call_reason == DLL_PROCESS_ATTACH {
    // A console may be useful for printing to 'stdout'
    // winapi::um::consoleapi::AllocConsole();

    // Preferably a thread should be created here instead, since as few
    // operations as possible should be performed within `DllMain`.
    main().is_ok() as BOOL
  } else {
    TRUE
  }
}
