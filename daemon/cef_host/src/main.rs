//! ozmux cef_host — out-of-process CEF browser host for the ozmux daemon.
//!
//! Lifecycle (mirrors examples/spike_init.rs Phase 0 verified pattern):
//!   1. Load CEF framework dylib on macOS
//!   2. Parse args + helper detection via CefExecuteProcess
//!   3. cef::api_hash (required when passing a CefApp to initialize)
//!   4. Build CefSettings with framework/resources/subprocess paths
//!   5. Wrap a minimal BrowserApp that injects no-sandbox / disable-gpu flags
//!   6. CefInitialize
//!   7. Spawn Tokio runtime on a background thread hosting the UDS control plane
//!   8. cef::run_message_loop() — blocks until QuitTask calls quit_message_loop()
//!   9. CefShutdown

use cef::Settings;
use cef::args::Args;
use cef::rc::Rc as _;
use cef::{App, ImplApp, ImplCommandLine, WrapApp, wrap_app};
use ozmux_browser_cef_protocol::wire::HostEvent;
use ozmux_cef_host::{control, pool, post_command};
use std::path::PathBuf;

// NOTE: BrowserApp injects --no-sandbox + --disable-gpu at command-line processing time.
// CEF runs multi-process: helper processes (renderer, gpu, network) are spawned from the
// cef_helper binary that sits next to cef_host (see browser_subprocess_path below).
// Multi-process is required for per-browser CefRequestContext objects to take effect.
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
            // command line here.
            let is_browser = process_type
                .map(|s| s.to_string().is_empty())
                .unwrap_or(true);
            if let (Some(cl), true) = (command_line, is_browser) {
                // NOTE: helper processes (GPU, Renderer, Network) run out-of-process via the
                // cef_helper binary.  Per-browser CefRequestContext objects are honored only
                // in this multi-process mode; --single-process made CEF ignore them.
                let flag2 = cef::CefString::from("no-sandbox");
                cl.append_switch(Some(&flag2));
                let flag3 = cef::CefString::from("disable-gpu");
                cl.append_switch(Some(&flag3));

                // NOTE: --use-mock-keychain makes Chromium's NetworkService use
                // an in-memory keychain for cookie encryption instead of the
                // macOS Keychain.  Without it, cef_host raises a "Chromium Safe
                // Storage" authorization dialog on launch.  ozmux seeds the
                // cookie store explicitly, so the real Keychain is never needed.
                let flag4 = cef::CefString::from("use-mock-keychain");
                cl.append_switch(Some(&flag4));

                // NOTE: Site Isolation is OFF by default to keep cef-rs 0.7 CDP
                // sessions stable (cross-origin nav otherwise tears down the
                // CDP session that holds viewport / input forwarding). Opt back
                // in by setting OZMUX_BROWSER_SITE_ISOLATION=1 — the env is
                // documented in CLAUDE.md for the chromiumoxide path and Plan 2
                // B15 brings it to the cef path verbatim.
                if std::env::var("OZMUX_BROWSER_SITE_ISOLATION").as_deref() != Ok("1") {
                    let disable_features = cef::CefString::from("disable-features");
                    let value = cef::CefString::from("IsolateOrigins,site-per-process");
                    cl.append_switch_with_value(Some(&disable_features), Some(&value));
                    let dsit = cef::CefString::from("disable-site-isolation-trials");
                    cl.append_switch(Some(&dsit));
                } else {
                    tracing::info!(
                        "OZMUX_BROWSER_SITE_ISOLATION=1 — Site Isolation left enabled"
                    );
                }
            }
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("OZMUX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[expect(
    clippy::field_reassign_with_default,
    reason = "macOS path fields set conditionally via cfg-guarded assignments below; struct-literal form is impractical"
)]
fn main() -> std::process::ExitCode {
    // ① macOS dylib load (Phase 0 finding: required before any CEF call)
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

        // NOTE: cef_api_hash must be called after loading the library and before any CEF call
        // that wraps a client-side struct (like CefApp).  It configures the DLL's internal
        // API version state; without it, CefInitialize with a non-null CefApp crashes with
        // "invalid version -1" because cef_api_version() returns -1.
        cef::api_hash(cef_dll_sys::CEF_API_VERSION, 0);
    }

    // ② Helper detection: returns -1 for browser process, >=0 for helper subprocess.
    let args = Args::new();
    let exit_code = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if exit_code >= 0 {
        return std::process::ExitCode::from(exit_code as u8);
    }

    // Browser process continues from here.
    init_tracing();
    tracing::info!(
        "cef_host browser process starting (pid={})",
        std::process::id()
    );

    // ③ CefSettings with paths
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
        let framework_dylib = cef_dir
            .join(FRAMEWORK_PATH)
            .canonicalize()
            .expect("failed to resolve CEF framework dylib path");
        let framework_dir = framework_dylib
            .parent()
            .expect("framework dylib has no parent")
            .to_path_buf();
        let resources_dir = framework_dir.join("Resources");

        settings.framework_dir_path =
            cef::CefString::from(framework_dir.to_string_lossy().as_ref());
        settings.resources_dir_path =
            cef::CefString::from(resources_dir.to_string_lossy().as_ref());
        // NOTE: on macOS, locales live in <Resources>/<locale>.lproj/locale.pak, not a
        // flat locales/ subdirectory, so locales_dir_path is left at default (empty).

        // Point browser_subprocess_path at the cef_helper binary so CEF can spawn GPU /
        // Renderer helper processes.  The binary must be on disk before CefInitialize.
        let helper_path = std::env::current_exe()
            .expect("cannot determine current exe path")
            .parent()
            .expect("exe has no parent dir")
            .join("cef_helper");
        settings.browser_subprocess_path =
            cef::CefString::from(helper_path.to_string_lossy().as_ref());

        tracing::info!("framework_dir_path = {}", framework_dir.display());
        tracing::info!("resources_dir_path = {}", resources_dir.display());
        tracing::info!("browser_subprocess_path = {}", helper_path.display());
    }

    // ④ Build BrowserApp and CefInitialize
    let mut app = BrowserApp::new();
    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    assert_eq!(ok, 1, "CefInitialize failed (return value: {ok})");
    tracing::info!("CefInitialize OK");

    // ⑤ PoolHandle + Tokio worker hosting the UDS control plane.
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<HostEvent>();
    // NOTE: event_tx is cloned into each BrowserPool entry's NavInner so that
    // DisplayHandler / LoadHandler can emit HostEvent::NavStateChanged to the
    // daemon without acquiring the pool lock.
    // TODO: Task 9 wires the real browser data root here.
    let handle = post_command::PoolHandle::new(pool::BrowserPool::new(
        event_tx,
        std::env::temp_dir(),
    ));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("cef-host-tokio")
        .build()
        .expect("build tokio runtime");
    let rt_handle = rt.handle().clone();

    let socket_path: PathBuf = std::env::var("OZMUX_CEF_HOST_SOCKET")
        .map(Into::into)
        .unwrap_or_else(|_| "/tmp/ozmux_cef_host.sock".into());
    let handle_for_control = handle.clone();
    let socket_for_log = socket_path.clone();
    std::thread::Builder::new()
        .name("cef-host-tokio".into())
        .spawn(move || {
            rt_handle.block_on(async move {
                tracing::info!(socket = %socket_for_log.display(), "control loop starting");
                match control::run(socket_path, handle_for_control, event_rx).await {
                    Ok(()) => tracing::info!("control loop closed normally"),
                    Err(e) => tracing::warn!(error = %e, "control loop failed; shutting down"),
                }
                // NOTE: when the control loop exits — gracefully or otherwise —
                // post a QuitTask so the main thread's CefRunMessageLoop returns
                // instead of sitting idle waiting for the next command.
                if let Err(e) = post_command::post_quit_loop() {
                    tracing::warn!(error = %e, "post_quit_loop failed; main may hang until SIGINT");
                }
            });
        })
        .expect("spawn tokio thread");

    tracing::info!("CefRunMessageLoop start");
    cef::run_message_loop();
    tracing::info!("CefRunMessageLoop returned (quit posted)");

    // ⑦ CefShutdown
    cef::shutdown();
    tracing::info!("CefShutdown OK");

    // NOTE: dropping the runtime here cancels the control loop task; the
    // event_tx parked in main is dropped just after, closing the event channel.
    drop(rt);
    std::process::ExitCode::SUCCESS
}
