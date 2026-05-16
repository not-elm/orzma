//! Phase 0 Task 4: verify CefInitialize + message loop polling + CefShutdown on macOS arm64.

use cef::Settings;
use cef::args::Args;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn main() {
    // NOTE: on macOS the CEF framework must be explicitly loaded via dlopen before any
    // CEF API call. Without this step any CEF call crashes (SIGSEGV) because the
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

    // CEF: returns -1 for browser process, >=0 for helper subprocess.
    let args = Args::new();
    let exit_code = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if exit_code >= 0 {
        std::process::exit(exit_code);
    }

    let mut settings = Settings::default();
    settings.windowless_rendering_enabled = 1;
    settings.no_sandbox = 1;
    settings.multi_threaded_message_loop = 0;
    settings.external_message_pump = 0;

    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        None,
        std::ptr::null_mut(),
    );
    assert_eq!(ok, 1, "CefInitialize failed (return value: {ok})");
    println!("CefInitialize OK");

    // Run message loop with polling; a background thread requests exit after 2 s.
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        stop2.store(true, Ordering::Release);
        println!("[bg] requesting quit");
    });
    println!("Message loop start");
    while !stop.load(Ordering::Acquire) {
        cef::do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("Message loop exited");

    cef::shutdown();
    println!("CefShutdown OK");
}
