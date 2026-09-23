//! Detours a function as soon as the library is loaded, before the host
//! program's `main` runs, so no invocation is missed.
//!
//! The library installs a detour of the function returning the process ID
//! (`getpid` or `GetCurrentProcessId`), which `early_target` calls first thing
//! in `main`. Build both examples, then load the library into the target:
//!
//! ```sh
//! $ cargo build --example early_hook --example early_target
//!
//! # Linux (and other ELF platforms)
//! $ LD_PRELOAD=target/debug/examples/libearly_hook.so target/debug/examples/early_target
//!
//! # macOS (not for SIP-protected or hardened-runtime binaries)
//! $ DYLD_INSERT_LIBRARIES=target/debug/examples/libearly_hook.dylib target/debug/examples/early_target
//!
//! # Windows (see the `launch_suspended` example)
//! $ cargo build --example launch_suspended
//! $ target\debug\examples\launch_suspended.exe target\debug\examples\early_hook.dll target\debug\examples\early_target.exe
//! ```
//!
//! Each prints `[early_hook] intercepted the process ID query` (possibly more
//! than once, if the runtime queries the process ID before `main`), followed
//! by the output of the target.
#![cfg(any(unix, windows))]

use detour::{Function, static_detour};
use std::io::Write;

#[cfg(unix)]
type ProcessId = libc::pid_t;
#[cfg(windows)]
type ProcessId = u32;

// `getpid` (Unix) or `GetCurrentProcessId` (Windows)
type FnProcessId = unsafe extern "system" fn() -> ProcessId;

static_detour! {
  static ProcessIdHook: unsafe extern "system" fn() -> ProcessId;
}

/// Called instead of the original function.
fn process_id_detour() -> ProcessId {
  // `write_all` avoids panicking if `stderr` is unavailable
  let _ = std::io::stderr().write_all(b"[early_hook] intercepted the process ID query\n");
  // SAFETY: The original function has no preconditions.
  unsafe { ProcessIdHook.call() }
}

/// Installs the detour; called when the library is loaded.
fn install() -> Result<(), Box<dyn std::error::Error>> {
  // SAFETY: The symbol refers to the process ID function, whose signature is
  // declared above, and no other threads are executing it (the process is
  // still being initialized).
  unsafe {
    let target = FnProcessId::from_ptr(process_id_function()?);
    ProcessIdHook
      .initialize(target, process_id_detour)?
      .enable()?;
  }
  Ok(())
}

/// Returns the address of the process ID function, as used by other modules.
///
/// The address must be resolved dynamically, since a direct reference could
/// resolve to a stub local to this library (e.g. a PLT entry, or an import
/// thunk), which other modules never call.
fn process_id_function() -> Result<*const (), &'static str> {
  #[cfg(unix)]
  // SAFETY: The symbol name is null-terminated.
  let address = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"getpid".as_ptr()) }.cast_const();

  #[cfg(windows)]
  // SAFETY: Both names are null-terminated.
  let address = unsafe {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let module = "kernel32.dll\0".encode_utf16().collect::<Vec<u16>>();
    GetProcAddress(
      GetModuleHandleW(module.as_ptr()),
      c"GetCurrentProcessId".as_ptr().cast(),
    )
    .map_or(std::ptr::null(), |function| function as *const ())
  };

  if address.is_null() {
    Err("could not resolve the process ID function")
  } else {
    Ok(address.cast())
  }
}

/// Reports the result of `install`; initialization cannot be aborted.
fn install_or_report() {
  if let Err(error) = install() {
    let _ = writeln!(std::io::stderr(), "[early_hook] failed to install: {error}");
  }
}

// Executed by the dynamic loader when the library is loaded, before `main`
#[cfg(unix)]
#[used]
#[cfg_attr(
  target_vendor = "apple",
  unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(not(target_vendor = "apple"), unsafe(link_section = ".init_array"))]
static CONSTRUCTOR: extern "C" fn() = {
  extern "C" fn constructor() {
    install_or_report();
  }
  constructor
};

#[cfg(windows)]
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
unsafe extern "system" fn DllMain(
  _module: windows_sys::Win32::Foundation::HINSTANCE,
  reason: u32,
  _reserved: *mut std::ffi::c_void,
) -> windows_sys::core::BOOL {
  if reason == windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH {
    install_or_report();
  }
  windows_sys::Win32::Foundation::TRUE
}
