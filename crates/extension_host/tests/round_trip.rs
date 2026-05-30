//! End-to-end: spawn the real `extensions/hello` Node extension and drive the
//! `ozmux-ext://` scheme handler against it. Requires the `cef` feature, `node`
//! on PATH, and `pnpm install` having linked `@ozmux/sdk`.
#![cfg(feature = "cef")]

use bevy_cef_core::prelude::{CefSchemeBody, CefSchemeHandler, CefSchemeRequest};
use ozmux_extension_host::scheme::OzmuxExtScheme;
use ozmux_extension_host::{ExtensionConfig, ExtensionHost};
use std::path::PathBuf;
use std::time::Duration;

fn hello_dir() -> PathBuf {
    // crates/extension_host/tests -> repo root -> extensions/hello
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/hello")
}

#[test]
fn serves_index_and_404_through_the_scheme_handler() {
    let cfg = ExtensionConfig::node("hello", hello_dir(), "index.ts");
    let host = ExtensionHost::spawn(cfg).expect("spawn hello");
    host.wait_ready(Duration::from_secs(10))
        .expect("hello ready");

    let scheme = OzmuxExtScheme::new("hello", host.endpoints());

    let ok = scheme.handle(&CefSchemeRequest {
        url: "ozmux-ext://hello/index.html".into(),
    });
    assert_eq!(ok.status, 200);
    assert_eq!(ok.mime_type, "text/html");
    let bytes = match ok.body {
        CefSchemeBody::Bytes(b) => b,
        _ => panic!("expected Bytes body"),
    };
    assert!(String::from_utf8_lossy(&bytes).contains("Hello from an ozmux extension"));

    let missing = scheme.handle(&CefSchemeRequest {
        url: "ozmux-ext://hello/missing".into(),
    });
    assert_eq!(missing.status, 404);
}
