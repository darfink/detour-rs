//! A `MessageBoxW` detour example.
//!
//! Ensure the crate is compiled as a 'cdylib' library to allow C interop, and
//! inject the library into a process (e.g. using a DLL injector).
#![cfg(windows)]

use detour::static_detour;
use std::error::Error;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, TRUE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::UI::WindowsAndMessaging::{MESSAGEBOX_RESULT, MESSAGEBOX_STYLE};
use windows_sys::core::{BOOL, PCWSTR};

// A type alias for `MessageBoxW` (makes the transmute easy on the eyes)
type FnMessageBoxW =
  unsafe extern "system" fn(HWND, PCWSTR, PCWSTR, MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT;

static_detour! {
  static MessageBoxWHook: unsafe extern "system" fn(HWND, PCWSTR, PCWSTR, MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT;
}

/// Called when the DLL is attached to the process.
unsafe fn main() -> Result<(), Box<dyn Error>> {
  // Retrieve an absolute address of `MessageBoxW`. This is required for
  // libraries due to the import address table. If `MessageBoxW` would be
  // provided directly as the target, it would only hook this DLL's
  // `MessageBoxW`. Using the method below an absolute address is retrieved
  // instead, detouring all invocations of `MessageBoxW` in the active process.
  let address = get_module_symbol_address("user32.dll", c"MessageBoxW")
    .ok_or("could not find 'MessageBoxW' address")?;

  // SAFETY: The symbol refers to `MessageBoxW`, whose signature is known.
  unsafe {
    let target: FnMessageBoxW = std::mem::transmute(address);

    // Initialize AND enable the detour (the 2nd parameter can also be a closure)
    MessageBoxWHook
      .initialize(target, messageboxw_detour)?
      .enable()?;
  }
  Ok(())
}

/// Called whenever `MessageBoxW` is invoked in the process.
fn messageboxw_detour(
  hwnd: HWND,
  text: PCWSTR,
  _caption: PCWSTR,
  style: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
  // Call the original `MessageBoxW`, but replace the caption
  let replaced_caption = "Detoured!\0".encode_utf16().collect::<Vec<u16>>();
  // SAFETY: The arguments are forwarded, and the caption is null-terminated.
  unsafe { MessageBoxWHook.call(hwnd, text, replaced_caption.as_ptr(), style) }
}

/// Returns a module symbol's absolute address.
fn get_module_symbol_address(module: &str, symbol: &std::ffi::CStr) -> Option<*const c_void> {
  let module = module
    .encode_utf16()
    .chain(std::iter::once(0))
    .collect::<Vec<u16>>();

  // SAFETY: Both strings are null-terminated.
  unsafe {
    let handle = GetModuleHandleW(module.as_ptr());
    GetProcAddress(handle, symbol.as_ptr().cast()).map(|function| function as *const c_void)
  }
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
unsafe extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
  if reason == DLL_PROCESS_ATTACH {
    // A console may be useful for printing to 'stdout'
    // windows_sys::Win32::System::Console::AllocConsole();

    // Preferably a thread should be created here instead, since as few
    // operations as possible should be performed within `DllMain`.
    // SAFETY: Called once, upon attachment.
    unsafe { main() }.is_ok() as BOOL
  } else {
    TRUE
  }
}
