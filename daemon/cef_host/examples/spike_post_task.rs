//! Phase A Task 1 spike: verify cef::post_task(WrapTask) round-trip.
//!
//! Boots CEF (single-process, no-sandbox, --disable-gpu, framework path resolved
//! via cef_dll_sys::get_cef_dir like main.rs), spawns a background thread that
//! enqueues 5 no-op tasks via cef::post_task, the CEF UI thread drains them via
//! the message loop. Each task increments a counter; main asserts counter == 5
//! before shutdown.
//!
//! Run:  cargo run --example spike_post_task -p ozmux_cef_host
//!
//! NOTE: API drift vs spec — the task description showed manual WrapTask/ImplTask
//! impls with an inner RcImpl field.  cef-rs 148 exposes wrap_task! macro
//! (mirrors wrap_app!) which generates the struct, WrapTask, ImplTask, Clone and
//! Rc impls automatically.  CountTask::new() returns a cef::Task directly.
//! ThreadId variant is ThreadId::UI (not ThreadId::Ui).  The spec's hand-rolled
//! impl pattern does not compile; the macro pattern used here is what the cef-rs
//! 148 source (resource_manager.rs, message_router.rs) actually shows.

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, ImplApp, ImplCommandLine, ImplTask, Settings, Task, ThreadId, WrapApp, WrapTask,
    post_task, wrap_app, wrap_task,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static EXEC_COUNT: AtomicUsize = AtomicUsize::new(0);

wrap_app! {
    struct SpikeApp;
    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&cef::CefString>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            let is_browser = process_type.map(|s| s.to_string().is_empty()).unwrap_or(true);
            if let (Some(cl), true) = (command_line, is_browser) {
                cl.append_switch(Some(&cef::CefString::from("single-process")));
                cl.append_switch(Some(&cef::CefString::from("no-sandbox")));
                cl.append_switch(Some(&cef::CefString::from("disable-gpu")));
            }
        }
    }
}

wrap_task! {
    struct CountTask;
    impl Task {
        fn execute(&self) {
            EXEC_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[expect(
    clippy::field_reassign_with_default,
    reason = "macOS path fields set conditionally via cfg-guarded assignments; struct-literal form is impractical"
)]
fn cef_init() {
    #[cfg(target_os = "macos")]
    {
        use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
        use std::os::unix::ffi::OsStrExt;
        let cef_dir = get_cef_dir().expect("CEF dir");
        let path = std::ffi::CString::new(
            cef_dir
                .join(FRAMEWORK_PATH)
                .canonicalize()
                .unwrap()
                .as_os_str()
                .as_bytes(),
        )
        .unwrap();
        let ok = unsafe { cef_dll_sys::cef_load_library(path.as_ptr().cast()) };
        assert_eq!(ok, 1, "cef_load_library failed");
        cef::api_hash(cef_dll_sys::CEF_API_VERSION, 0);
    }

    let args = Args::new();
    let exit = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    assert!(exit < 0, "browser process expected (got exit_code={exit})");

    let mut settings = Settings::default();
    settings.windowless_rendering_enabled = 1;
    settings.no_sandbox = 1;
    settings.multi_threaded_message_loop = 0;
    settings.external_message_pump = 0;

    #[cfg(target_os = "macos")]
    {
        use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
        let cef_dir = get_cef_dir().unwrap();
        let framework_dylib = cef_dir.join(FRAMEWORK_PATH).canonicalize().unwrap();
        let framework_dir = framework_dylib.parent().unwrap().to_path_buf();
        settings.framework_dir_path =
            cef::CefString::from(framework_dir.to_string_lossy().as_ref());
        settings.resources_dir_path =
            cef::CefString::from(framework_dir.join("Resources").to_string_lossy().as_ref());
        let helper = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join("cef_helper");
        settings.browser_subprocess_path = cef::CefString::from(helper.to_string_lossy().as_ref());
    }

    let mut app = SpikeApp::new();
    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    assert_eq!(ok, 1, "CefInitialize failed");
}

fn main() {
    cef_init();

    // NOTE: CountTask::new() returns a cef::Task (not CountTask directly) per the
    // wrap_task! macro expansion; post_task takes &mut Task.
    std::thread::spawn(|| {
        for _ in 0..5 {
            let mut task = CountTask::new();
            let posted = post_task(ThreadId::UI, Some(&mut task));
            assert_eq!(posted, 1, "post_task returned 0 (expected 1)");
        }
    });

    let start = Instant::now();
    while EXEC_COUNT.load(Ordering::SeqCst) < 5 {
        cef::do_message_loop_work();
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "tasks did not drain in 5s"
        );
    }
    println!(
        "Spike OK: {} tasks executed on UI thread",
        EXEC_COUNT.load(Ordering::SeqCst)
    );
    cef::shutdown();
}
