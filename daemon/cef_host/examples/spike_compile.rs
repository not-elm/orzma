//! Verifies CefExecuteProcess returns -1 for the browser process.
//! On macOS, cef_load_library must be called first to dlopen the CEF framework.

use cef::args::Args;

fn main() {
    // NOTE: on macOS the CEF framework must be explicitly loaded via dlopen before any
    // CEF API call.  Without this step cef_execute_process crashes (SIGSEGV) because the
    // stub wrapper's function-pointer table is still zeroed.
    #[cfg(target_os = "macos")]
    {
        use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
        use std::os::unix::ffi::OsStrExt;
        let cef_dir = get_cef_dir().expect("CEF directory not found");
        let framework = cef_dir
            .join(FRAMEWORK_PATH)
            .canonicalize()
            .expect("failed to resolve CEF framework path");
        let path =
            std::ffi::CString::new(framework.as_os_str().as_bytes()).expect("invalid path bytes");
        let ok = unsafe { cef_dll_sys::cef_load_library(path.as_ptr().cast()) };
        assert_eq!(ok, 1, "cef_load_library failed — framework missing?");
        println!("cef_load_library OK: {}", framework.display());
    }

    let args = Args::new();
    // CEF: returns -1 for browser process, >=0 for helper subprocess.
    let exit_code = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    println!("CefExecuteProcess returned: {exit_code}");
    if exit_code >= 0 {
        std::process::exit(exit_code);
    }
    println!("Browser process path — CefExecuteProcess returned -1 as expected");
}
