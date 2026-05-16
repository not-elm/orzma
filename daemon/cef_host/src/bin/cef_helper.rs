//! cef_host helper binary — runs in CEF subprocesses (GPU / Renderer / Plugin / Utility).
//! Must call CefExecuteProcess and exit immediately with the returned code.

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
        if ok != 1 {
            // NOTE: helper cannot safely print to stderr once CEF intercepts the process,
            // but we try before any CEF initialisation.
            eprintln!("cef_load_library failed — framework missing?");
            std::process::exit(1);
        }
    }

    let args = Args::new();
    let exit_code = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    std::process::exit(if exit_code >= 0 { exit_code } else { 1 });
}
