//! cef_host helper binary — runs in CEF subprocesses (GPU / Renderer / Plugin / Utility).
//! Must call CefExecuteProcess and exit immediately with the returned code.
//!
//! Registering an `App` in the helper exists solely so
//! `on_before_command_line_processing` can inject `--use-mock-keychain` into
//! every helper process. CEF does not propagate that switch from the browser
//! command line, so without this the Network Service utility falls back to the
//! real macOS Keychain and pops a "Chromium Safe Storage" authorization dialog.

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{App, ImplApp, ImplCommandLine, WrapApp, wrap_app};

wrap_app! {
    struct HelperApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&cef::CefString>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            if let Some(cl) = command_line {
                let mock_kc = cef::CefString::from("use-mock-keychain");
                cl.append_switch(Some(&mock_kc));
            }
        }
    }
}

fn main() {
    // NOTE: on macOS the CEF framework must be explicitly loaded via dlopen before any
    // CEF API call.  Without this step cef_execute_process crashes (SIGSEGV) because the
    // stub wrapper's function-pointer table is still zeroed.
    #[cfg(target_os = "macos")]
    {
        use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
        use std::os::unix::ffi::OsStrExt;
        let exe = std::env::current_exe().expect("current_exe");
        let in_bundle = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|frameworks| {
                frameworks.join("Chromium Embedded Framework.framework/Chromium Embedded Framework")
            })
            .filter(|p| p.exists());
        let framework = match in_bundle {
            Some(p) => p
                .canonicalize()
                .expect("failed to resolve in-bundle framework path"),
            None => {
                let cef_dir = get_cef_dir().expect("CEF directory not found");
                cef_dir
                    .join(FRAMEWORK_PATH)
                    .canonicalize()
                    .expect("failed to resolve CEF framework path")
            }
        };
        let path =
            std::ffi::CString::new(framework.as_os_str().as_bytes()).expect("invalid path bytes");
        let ok = unsafe { cef_dll_sys::cef_load_library(path.as_ptr().cast()) };
        if ok != 1 {
            eprintln!("cef_load_library failed — framework missing?");
            std::process::exit(1);
        }

        // NOTE: cef_api_hash must be called after cef_load_library and before any CEF
        // call that wraps a client-side struct (like the HelperApp passed to
        // cef::execute_process below). Without it the wrapper's function-pointer table
        // is uninitialised and execute_process FATALs with
        // "CefApp_0_CToCpp called with invalid version -1".
        cef::api_hash(cef_dll_sys::CEF_API_VERSION, 0);
    }

    let args = Args::new();
    let mut app = HelperApp::new();
    let exit_code = cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    std::process::exit(if exit_code >= 0 { exit_code } else { 1 });
}
