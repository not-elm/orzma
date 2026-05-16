//! Phase 0 Task 5: resolve CEF resource paths at runtime so CefInitialize succeeds without
//! a .app bundle.  Sets framework_dir_path, resources_dir_path, and browser_subprocess_path
//! in Settings before calling CefInitialize.

use cef::Settings;
use cef::args::Args;
use cef::{App, ImplApp, ImplCommandLine, WrapApp, wrap_app};
use cef::rc::Rc as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// NOTE: BrowserApp injects --single-process + --no-sandbox + --disable-gpu at command-line
// processing time.  Without a proper .app bundle layout the GPU / Renderer helper processes
// crash immediately (icudtl.dat not found); --single-process avoids spawning them entirely.
wrap_app! {
    struct BrowserApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&cef::CefString>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            // NOTE: process_type is empty string "" for the browser process and non-empty
            // for helper processes (renderer, gpu, etc.).  Only modify the browser process
            // command line here; helper processes use what the browser process spawns them
            // with.
            let is_browser = process_type
                .map(|s| s.to_string().is_empty())
                .unwrap_or(true);
            if let (Some(cl), true) = (command_line, is_browser) {
                // NOTE: --single-process collapses all helper processes (GPU, Renderer,
                // Network) into the browser process, avoiding subprocess bundle requirements
                // on macOS.  Required for running without a proper .app bundle layout.
                let flag = cef::CefString::from("single-process");
                cl.append_switch(Some(&flag));
                let flag2 = cef::CefString::from("no-sandbox");
                cl.append_switch(Some(&flag2));
                let flag3 = cef::CefString::from("disable-gpu");
                cl.append_switch(Some(&flag3));
            }
        }
    }
}

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

        // NOTE: cef_api_hash must be called after loading the library and before any CEF call
        // that wraps a client-side struct (like CefApp).  It configures the DLL's internal
        // API version state; without it, CefInitialize with a non-null CefApp crashes with
        // "invalid version -1" because cef_api_version() returns -1.
        cef::api_hash(cef_dll_sys::CEF_API_VERSION, 0);
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

    // NOTE: on macOS, CefInitialize requires explicit paths to the framework and its resources.
    // Without these, CEF fails immediately with "icudtl.dat not found in bundle".
    #[cfg(target_os = "macos")]
    {
        use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
        let cef_dir = get_cef_dir().expect("CEF directory not found");
        // FRAMEWORK_PATH = "Chromium Embedded Framework.framework/Chromium Embedded Framework"
        // so parent() gives us the .framework bundle directory itself.
        let framework_dylib = cef_dir.join(FRAMEWORK_PATH).canonicalize()
            .expect("failed to resolve CEF framework dylib path");
        let framework_dir = framework_dylib.parent()
            .expect("framework dylib has no parent")
            .to_path_buf();
        let resources_dir = framework_dir.join("Resources");

        settings.framework_dir_path = cef::CefString::from(
            framework_dir.to_string_lossy().as_ref(),
        );
        settings.resources_dir_path = cef::CefString::from(
            resources_dir.to_string_lossy().as_ref(),
        );
        // NOTE: on macOS, locales live in <Resources>/<locale>.lproj/locale.pak, not a
        // flat locales/ subdirectory, so locales_dir_path is left at default (empty).

        // Point browser_subprocess_path at the cef_helper binary so CEF can spawn GPU /
        // Renderer helper processes.  The binary must be on disk before CefInitialize.
        let helper_path = std::env::current_exe()
            .expect("cannot determine current exe path")
            .parent()
            .expect("exe has no parent dir")
            .join("cef_helper");
        settings.browser_subprocess_path = cef::CefString::from(
            helper_path.to_string_lossy().as_ref(),
        );

        println!("framework_dir_path  = {}", framework_dir.display());
        println!("resources_dir_path  = {}", resources_dir.display());
        println!("browser_subprocess_path = {}", helper_path.display());
    }

    let mut app = BrowserApp::new();
    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
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
