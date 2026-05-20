//! CEF initialization-time settings construction and macOS bundle path
//! resolution. Exposes helpers used by the daemon `main` to build the
//! `cef::Settings` passed to `cef::initialize`, load the CEF framework dylib,
//! and acquire the cross-process data-root lock.

use cef::Settings;
use std::path::{Path, PathBuf};

use crate::profile::{self, DataRootLock};

/// Builds the `Settings` passed to `cef::initialize`.
#[expect(
    clippy::field_reassign_with_default,
    reason = "macOS path fields set conditionally via cfg-guarded assignments; struct-literal form is impractical"
)]
pub fn build_cef_settings(browser_data_root: &Path) -> Settings {
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

/// Loads the CEF framework dylib and arms CEF's API-version state.
///
/// macOS-only; a no-op on other platforms (CEF is statically linked there).
#[cfg(target_os = "macos")]
pub fn load_cef_framework() {
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

/// No-op stub on non-macOS targets (CEF is statically linked there).
#[cfg(not(target_os = "macos"))]
pub fn load_cef_framework() {}

/// Returns the in-bundle Chromium Embedded Framework dylib path when running
/// from inside `ozmux-daemon.app`, or `None` otherwise.
#[cfg(target_os = "macos")]
pub fn in_bundle_framework_dylib() -> Option<PathBuf> {
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

/// Returns the path to the generic helper executable inside
/// `ozmux-daemon.app`, or `None` if the current exe is not running from
/// inside such a bundle.
#[cfg(target_os = "macos")]
pub fn in_bundle_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    let helper = contents
        .join("Frameworks")
        .join("ozmux-daemon Helper.app")
        .join("Contents/MacOS")
        .join("ozmux-daemon Helper");
    helper.exists().then_some(helper)
}

/// Resolves the browser-data root and tries to acquire the cross-process lock.
///
/// The returned lock is `None` when another daemon already holds it; the
/// caller must keep the lock guard alive until after `run_message_loop`
/// returns so the OS lock is not released early.
pub fn acquire_data_root() -> (PathBuf, Option<DataRootLock>) {
    let browser_data_root = profile::browser_data_root();
    let data_root_lock = profile::acquire_data_root_lock(&browser_data_root)
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
    settings.browser_subprocess_path = cef::CefString::from(helper_path.to_string_lossy().as_ref());

    tracing::info!("framework_dir_path = {}", framework_dir.display());
    tracing::info!("resources_dir_path = {}", resources_dir.display());
    tracing::info!("browser_subprocess_path = {}", helper_path.display());
}

/// Returns the helper executable path: in-bundle when running from
/// `ozmux-daemon.app`, sibling `cef_helper` otherwise (legacy debug layout).
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
