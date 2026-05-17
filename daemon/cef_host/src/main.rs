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
use cef::{App, ImplApp, WrapApp, wrap_app};
use ozmux_browser_cef_protocol::wire::HostEvent;
use ozmux_cef_host::{append_flag, append_flag_value, control, pool, post_command};
use std::path::{Path, PathBuf};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedReceiver;

fn main() -> std::process::ExitCode {
    load_cef_framework();

    let args = Args::new();
    if let Some(code) = dispatch_helper_process_or_continue(&args) {
        return code;
    }

    init_tracing();
    tracing::info!(
        "cef_host browser process starting (pid={})",
        std::process::id()
    );

    let (browser_data_root, data_root_lock) = acquire_data_root();
    let settings = build_cef_settings(&browser_data_root);

    let mut app = BrowserApp::new();
    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    assert_eq!(ok, 1, "CefInitialize failed (return value: {ok})");
    tracing::info!("CefInitialize OK");

    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<HostEvent>();
    let handle = post_command::PoolHandle::new(pool::BrowserPool::new(
        event_tx,
        browser_data_root,
        data_root_lock.is_some(),
    ));
    // NOTE: keep the data-root lock alive for the whole of main — it releases
    // on drop, so it must outlive run_message_loop() below. Dropping early
    // releases the OS lock; concurrent daemons would then corrupt the data root.
    let _data_root_lock = data_root_lock;

    let rt = spawn_control_runtime(handle, event_rx);

    tracing::info!("CefRunMessageLoop start");
    cef::run_message_loop();
    tracing::info!("CefRunMessageLoop returned (quit posted)");

    cef::shutdown();
    tracing::info!("CefShutdown OK");

    drop(rt);
    std::process::ExitCode::SUCCESS
}

/// Loads the CEF framework dylib and arms CEF's API-version state.
///
/// macOS-only; a no-op on other platforms (CEF is statically linked there).
#[cfg(target_os = "macos")]
fn load_cef_framework() {
    use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
    use std::os::unix::ffi::OsStrExt;
    let framework = in_bundle_framework_dylib()
        .or_else(|| {
            let cef_dir = get_cef_dir()?;
            cef_dir.join(FRAMEWORK_PATH).canonicalize().ok()
        })
        .expect("failed to resolve CEF framework path");
    let path =
        std::ffi::CString::new(framework.as_os_str().as_bytes()).expect("invalid path bytes");
    // SAFETY: framework path was canonicalized and exists; CEF dylib loader
    // accepts a NUL-terminated UTF-8 byte string.
    let ok = unsafe { cef_dll_sys::cef_load_library(path.as_ptr().cast()) };
    assert_eq!(ok, 1, "cef_load_library failed — framework missing?");

    // NOTE: cef_api_hash must be called after loading the library and before any CEF call
    // that wraps a client-side struct (like CefApp). It configures the DLL's internal
    // API version state; without it, CefInitialize with a non-null CefApp crashes with
    // "invalid version -1" because cef_api_version() returns -1.
    cef::api_hash(cef_dll_sys::CEF_API_VERSION, 0);
}

#[cfg(not(target_os = "macos"))]
fn load_cef_framework() {}

/// Returns the in-bundle Chromium Embedded Framework dylib path when running
/// from inside `cef_host.app`, or `None` otherwise.
#[cfg(target_os = "macos")]
fn in_bundle_framework_dylib() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    let dylib = contents
        .join("Frameworks")
        .join("Chromium Embedded Framework.framework")
        .join("Chromium Embedded Framework");
    dylib.exists().then_some(dylib)
}

/// Runs `cef::execute_process`. Returns `Some(code)` if this invocation is a
/// helper subprocess (caller must return that code immediately), or `None` if
/// the current process is the browser process and should continue.
fn dispatch_helper_process_or_continue(args: &Args) -> Option<std::process::ExitCode> {
    let exit_code = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if exit_code >= 0 {
        Some(std::process::ExitCode::from(exit_code as u8))
    } else {
        None
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("OZMUX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Resolves the browser-data root and tries to acquire the cross-process lock.
///
/// The returned lock is `None` when another daemon already holds it; the
/// caller must keep the lock guard alive until after `run_message_loop`
/// returns so the OS lock is not released early.
fn acquire_data_root() -> (PathBuf, Option<ozmux_cef_host::profile::DataRootLock>) {
    let browser_data_root = ozmux_cef_host::profile::browser_data_root();
    let data_root_lock = ozmux_cef_host::profile::acquire_data_root_lock(&browser_data_root)
        .expect("create browser data root");
    if data_root_lock.is_none() {
        tracing::warn!(
            root = %browser_data_root.display(),
            "another daemon holds the browser data root; named profiles disabled — \
             all Browser Activities will use incognito storage"
        );
    }
    (browser_data_root, data_root_lock)
}

/// Builds the `Settings` passed to `cef::initialize`.
#[expect(
    clippy::field_reassign_with_default,
    reason = "macOS path fields set conditionally via cfg-guarded assignments; struct-literal form is impractical"
)]
fn build_cef_settings(browser_data_root: &Path) -> Settings {
    let mut settings = Settings::default();
    settings.windowless_rendering_enabled = 1;
    settings.root_cache_path = cef::CefString::from(browser_data_root.to_string_lossy().as_ref());
    settings.no_sandbox = 1;
    settings.multi_threaded_message_loop = 0;
    settings.external_message_pump = 0;

    #[cfg(target_os = "macos")]
    apply_macos_settings(&mut settings);

    settings
}

/// Builds the macOS-specific portion of `Settings` — framework / resources /
/// subprocess paths. A no-op on other platforms.
#[cfg(target_os = "macos")]
fn apply_macos_settings(settings: &mut Settings) {
    use cef_dll_sys::{FRAMEWORK_PATH, get_cef_dir};
    let framework_dylib = in_bundle_framework_dylib()
        .or_else(|| {
            let cef_dir = get_cef_dir()?;
            cef_dir.join(FRAMEWORK_PATH).canonicalize().ok()
        })
        .expect("failed to resolve CEF framework dylib path");
    let framework_dir = framework_dylib
        .parent()
        .expect("framework dylib has no parent")
        .to_path_buf();
    let resources_dir = framework_dir.join("Resources");

    settings.framework_dir_path = cef::CefString::from(framework_dir.to_string_lossy().as_ref());
    settings.resources_dir_path = cef::CefString::from(resources_dir.to_string_lossy().as_ref());
    // NOTE: on macOS, locales live in <Resources>/<locale>.lproj/locale.pak, not a
    // flat locales/ subdirectory, so locales_dir_path is left at default (empty).

    let helper_path = resolve_browser_subprocess_path();
    settings.browser_subprocess_path =
        cef::CefString::from(helper_path.to_string_lossy().as_ref());

    tracing::info!("framework_dir_path = {}", framework_dir.display());
    tracing::info!("resources_dir_path = {}", resources_dir.display());
    tracing::info!("browser_subprocess_path = {}", helper_path.display());
}

/// Returns the helper executable path: in-bundle when running from
/// `cef_host.app`, sibling `cef_helper` otherwise (legacy debug layout).
#[cfg(target_os = "macos")]
fn resolve_browser_subprocess_path() -> PathBuf {
    in_bundle_helper().unwrap_or_else(|| {
        std::env::current_exe()
            .expect("cannot determine current exe path")
            .parent()
            .expect("exe has no parent dir")
            .join("cef_helper")
    })
}

/// Returns the path to the generic helper executable inside `cef_host.app`,
/// or `None` if the current exe is not running from inside such a bundle.
#[cfg(target_os = "macos")]
fn in_bundle_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    let helper = contents
        .join("Frameworks")
        .join("cef_host Helper.app")
        .join("Contents/MacOS")
        .join("cef_host Helper");
    helper.exists().then_some(helper)
}

wrap_app! {
    struct BrowserApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&cef::CefString>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            let is_browser = process_type
                .map(|s| s.to_string().is_empty())
                .unwrap_or(true);
            let Some(cl) = command_line else {
                return;
            };

            // NOTE: --use-mock-keychain must reach EVERY process. CEF does not always
            // propagate it from the browser command line to helpers, so the Network Service
            // utility (which performs cookie encryption) ends up invoking the real macOS
            // Keychain and raises a "Chromium Safe Storage" authorization dialog. Inject it
            // unconditionally to keep cookie crypto fully in-memory.
            append_flag(cl, "use-mock-keychain");

            if is_browser {
                append_flag(cl, "no-sandbox");
                append_flag(cl, "disable-gpu");

                if std::env::var("OZMUX_BROWSER_SITE_ISOLATION").as_deref() != Ok("1") {
                    append_flag_value(cl, "disable-features", "IsolateOrigins,site-per-process");
                    append_flag(cl, "disable-site-isolation-trials");
                } else {
                    tracing::info!(
                        "OZMUX_BROWSER_SITE_ISOLATION=1 — Site Isolation left enabled"
                    );
                }
            }
        }
    }
}

/// Builds the Tokio runtime, then spawns a named thread that drives the UDS
/// control plane on it. Returns the runtime so the caller can keep it alive
/// until `run_message_loop` returns.
fn spawn_control_runtime(
    handle: post_command::PoolHandle,
    event_rx: UnboundedReceiver<HostEvent>,
) -> Runtime {
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
    let socket_for_log = socket_path.clone();
    std::thread::Builder::new()
        .name("cef-host-tokio".into())
        .spawn(move || {
            rt_handle.block_on(async move {
                tracing::info!(socket = %socket_for_log.display(), "control loop starting");
                match control::run(socket_path, handle, event_rx).await {
                    Ok(()) => tracing::info!("control loop closed normally"),
                    Err(e) => tracing::warn!(error = %e, "control loop failed; shutting down"),
                }
                if let Err(e) = post_command::post_quit_loop() {
                    tracing::warn!(error = %e, "post_quit_loop failed; main may hang until SIGINT");
                }
            });
        })
        .expect("spawn tokio thread");
    rt
}
