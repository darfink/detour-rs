//! Launches a program with a library loaded before its entry point runs
//! (Windows).
//!
//! The process is created suspended, and a call to `LoadLibraryW` is queued
//! as an asynchronous procedure call (APC) on its main thread. Once resumed,
//! the APC executes during the loader's initialization, after the target's
//! static imports are loaded, but before its entry point (or `main`) runs.
//! The library can therefore install detours in `DllMain` without missing
//! any invocation from the program itself.
//!
//! See the `early_hook` example for usage. The launcher and the target must
//! share the same architecture (e.g. both x86-64), since the address of
//! `LoadLibraryW` is assumed to match (system libraries are mapped at the same
//! address in all processes of the same architecture).
//!
//! Libraries loaded before the program (and their `DllMain`) are not covered.
//! Microsoft Detours' `DetourCreateProcessWithDlls` instead adds the library
//! to the program's import table, so it loads before other dependencies.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
  use std::os::windows::ffi::OsStrExt;
  use windows_sys::Win32::Foundation::{CloseHandle, TRUE, WAIT_OBJECT_0};
  use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
  };
  use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
  use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
  use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx,
  };
  use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION,
    QueueUserAPC, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
  };

  fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
  }

  let mut arguments = std::env::args_os().skip(1);
  let (Some(library), Some(program)) = (arguments.next(), arguments.next()) else {
    return Err("usage: launch_suspended <library.dll> <program.exe> [arguments...]".into());
  };

  // The library path must be absolute, since the target's working directory
  // and search paths may differ.
  let library = wide(std::path::absolute(library)?.as_os_str());

  // Build the command line, quoting each argument
  let mut command_line = std::ffi::OsString::new();
  for (index, argument) in std::iter::once(program.clone())
    .chain(arguments)
    .enumerate()
  {
    if index > 0 {
      command_line.push(" ");
    }
    command_line.push("\"");
    command_line.push(argument);
    command_line.push("\"");
  }
  let mut command_line = wide(&command_line);

  // SAFETY: All pointers refer to valid, null-terminated strings and
  // initialized structures, and every handle is closed exactly once.
  unsafe {
    let mut startup: STARTUPINFOW = std::mem::zeroed();
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    startup.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
    startup.hStdError = GetStdHandle(STD_ERROR_HANDLE);

    let mut process: PROCESS_INFORMATION = std::mem::zeroed();
    if CreateProcessW(
      wide(&program).as_ptr(),
      command_line.as_mut_ptr(),
      std::ptr::null(),
      std::ptr::null(),
      TRUE,
      CREATE_SUSPENDED,
      std::ptr::null(),
      std::ptr::null(),
      &startup,
      &mut process,
    ) == 0
    {
      return Err(std::io::Error::last_os_error().into());
    }

    let inject = || -> std::io::Result<()> {
      // Copy the library path into the target process
      let size = library.len() * size_of::<u16>();
      let remote_path = VirtualAllocEx(
        process.hProcess,
        std::ptr::null(),
        size,
        MEM_COMMIT | MEM_RESERVE,
        PAGE_READWRITE,
      );
      if remote_path.is_null()
        || WriteProcessMemory(
          process.hProcess,
          remote_path,
          library.as_ptr().cast(),
          size,
          std::ptr::null_mut(),
        ) == 0
      {
        return Err(std::io::Error::last_os_error());
      }

      // `LoadLibraryW` is compatible with an APC routine: it accepts a single
      // pointer-sized argument, and uses the same calling convention.
      let kernel32 = wide("kernel32.dll".as_ref());
      let load_library = GetProcAddress(
        GetModuleHandleW(kernel32.as_ptr()),
        c"LoadLibraryW".as_ptr().cast(),
      )
      .ok_or_else(std::io::Error::last_os_error)?;
      let routine = std::mem::transmute::<
        unsafe extern "system" fn() -> isize,
        unsafe extern "system" fn(usize),
      >(load_library);

      if QueueUserAPC(Some(routine), process.hThread, remote_path as usize) == 0 {
        return Err(std::io::Error::last_os_error());
      }
      Ok(())
    };

    if let Err(error) = inject() {
      TerminateProcess(process.hProcess, 1);
      CloseHandle(process.hThread);
      CloseHandle(process.hProcess);
      return Err(error.into());
    }

    ResumeThread(process.hThread);
    CloseHandle(process.hThread);

    let mut exit_code = 1;
    if WaitForSingleObject(process.hProcess, INFINITE) == WAIT_OBJECT_0 {
      GetExitCodeProcess(process.hProcess, &mut exit_code);
    }
    CloseHandle(process.hProcess);
    std::process::exit(exit_code as i32);
  }
}

#[cfg(not(windows))]
fn main() {
  eprintln!("this example requires Windows; see the `early_hook` example for other platforms");
  std::process::exit(1);
}
