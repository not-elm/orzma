//! Phase A Task A12 spike: verify cef-rs 148 IME composition APIs.
//!
//! Boots CEF (single-process, no-sandbox, --disable-gpu, framework path resolved
//! via cef_dll_sys::get_cef_dir like spike_post_task.rs), creates a windowless
//! about:blank browser, then exercises three IME host methods on the CEF UI thread:
//!   1. `ime_set_composition` — text + underlines + replacement/selection ranges
//!   2. `ime_finish_composing_text` — keep_selection = 0
//!   3. `ime_cancel_composition`
//!
//! Run:  cargo run --example spike_ime -p ozmux_cef_host
//!
//! NOTE: API drift vs spec sketch —
//!   - `ime_finish_composing_text(keep_selection: c_int)` matches spec exactly.
//!   - The spec mentioned `(-1, -1)` for "no replacement range"; cef-rs 148 uses
//!     `Range { from: u32, to: u32 }` — both fields are unsigned, so there is no
//!     sentinel for "none".  `None` is passed for `replacement_range` instead.
//!   - `ime_commit_text` exists as a separate method (not mentioned in the spec).
//!     It takes `text`, `replacement_range`, and `relative_cursor_pos: c_int`.
//!   - `CompositionUnderline` has a `size` field that must equal
//!     `std::mem::size_of::<_cef_composition_underline_t>()`;
//!     `CompositionUnderline::default()` sets it correctly.
//!   - `Range` fields are `u32` (not `i32`); there is no "invalid/none" sentinel value.

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, BrowserSettings, CefString, CompositionUnderline, ImplApp, ImplBrowser, ImplBrowserHost,
    ImplCommandLine, ImplTask, Range, Settings, Task, ThreadId, WindowInfo, WrapApp, WrapTask,
    browser_host_create_browser_sync, post_task, wrap_app, wrap_task,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_cef_host::handlers::client::OzmuxClient;
use ozmux_cef_host::handlers::lifespan::OzmuxLifeSpanHandler;
use ozmux_cef_host::handlers::render::{OzmuxRenderHandler, RenderHandlerState};
use std::os::raw::c_int;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static DONE: AtomicBool = AtomicBool::new(false);

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

// NOTE: ImeTask holds a clone of the BrowserHost so the task can call IME methods
// on the UI thread. BrowserHost is a CEF ref-counted object that is safe to clone.
wrap_task! {
    struct ImeTask {
        host: cef::BrowserHost,
    }

    impl Task {
        fn execute(&self) {
            let text = CefString::from("こんにちは");

            // NOTE: CompositionUnderline::default() sets size to the correct
            // sizeof(_cef_composition_underline_t) required by CEF.
            let underline = CompositionUnderline {
                range: Range { from: 0, to: 5 },
                ..CompositionUnderline::default()
            };
            let underlines = [underline];
            let selection = Range { from: 0, to: 5 };

            // NOTE: replacement_range is None because cef-rs 148 Range uses u32
            // fields with no sentinel for "no replacement"; None is the correct way
            // to signal "no replacement range" in this API.
            std::thread::sleep(Duration::from_millis(500));
            self.host.ime_set_composition(
                Some(&text),
                Some(&underlines),
                None,
                Some(&selection),
            );

            std::thread::sleep(Duration::from_millis(300));
            // NOTE: keep_selection = 0 means "do not keep selection after composing ends".
            self.host.ime_finish_composing_text(0 as c_int);

            std::thread::sleep(Duration::from_millis(300));
            self.host.ime_cancel_composition();

            DONE.store(true, Ordering::SeqCst);
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

#[expect(
    clippy::field_reassign_with_default,
    reason = "WindowInfo::default() uses unsafe zeroed() with size field; struct-literal form is impractical due to raw pointer fields"
)]
fn create_browser() -> cef::Browser {
    let aid = ActivityId("spike-ime".to_string());
    let state = Arc::new(RenderHandlerState::new(100, 100, 1.0));

    // NOTE: ShmWriter is not needed for this spike — we pass a no-op shm stub.
    // OzmuxRenderHandler requires Arc<ShmWriter> from the production path.
    // Instead, we use a raw null mmap region to satisfy the type. Since on_paint
    // will not fire on about:blank in single-process mode before the task
    // completes, this is safe for the spike.
    //
    // SAFETY: We create a minimal anonymous mmap region to satisfy ShmWriter's
    // constructor. The region is never written to in production logic during this
    // spike because about:blank does not trigger on_paint before DONE is set.
    let total_size =
        ozmux_cef_host::shm_writer::ShmWriter::required_region_size(1280 * 800 * 4 + 4096);
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            total_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(ptr, libc::MAP_FAILED, "anonymous mmap failed");
    // SAFETY: ptr is a valid anonymous mmap region of total_size bytes, private.
    let shm = Arc::new(unsafe {
        ozmux_cef_host::shm_writer::ShmWriter::from_mmap(ptr as *mut u8, 1280 * 800 * 4 + 4096)
    });

    let render_handler = OzmuxRenderHandler::new(aid.clone(), shm, state);
    let life_span_handler = OzmuxLifeSpanHandler::new(aid);
    let mut client = OzmuxClient::new(render_handler, life_span_handler);

    let mut window_info = WindowInfo::default();
    window_info.windowless_rendering_enabled = 1;

    let browser_settings = BrowserSettings {
        windowless_frame_rate: 30,
        ..BrowserSettings::default()
    };
    let url = CefString::from("about:blank");

    browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&browser_settings),
        None,
        None,
    )
    .expect("browser_host_create_browser_sync returned None")
}

fn main() {
    cef_init();

    let browser = create_browser();
    let host = browser.host().expect("browser.host() returned None");

    // NOTE: Post the IME task to the UI thread from a background thread.
    // The task clones the host (CEF ref-counted) and runs IME calls sequentially
    // with sleeps interleaved. The main thread pumps the message loop until DONE.
    std::thread::spawn(move || {
        let mut task = ImeTask::new(host);
        let posted = post_task(ThreadId::UI, Some(&mut task));
        assert_eq!(posted, 1, "post_task returned 0");
    });

    let start = Instant::now();
    while !DONE.load(Ordering::SeqCst) {
        cef::do_message_loop_work();
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "IME task did not complete within 10s"
        );
    }

    println!("IME spike OK");
    cef::shutdown();
}
