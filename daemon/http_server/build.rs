//! Ensure `src/handlers/index.html` exists so `include_str!` succeeds even
//! when `vite build` has not run yet. The real bundle is written there by
//! the frontend build; this stub is only used on fresh checkouts and when
//! a debug daemon is invoked without first bundling the UI.

use std::fs;
use std::path::PathBuf;

const STUB: &str = concat!(
    "<!doctype html>\n",
    "<meta charset=\"utf-8\">\n",
    "<title>ozmux — frontend not bundled</title>\n",
    "<style>body{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem;line-height:1.5}code{background:#eee;padding:.1em .3em;border-radius:.2em}</style>\n",
    "<h1>ozmux</h1>\n",
    "<p>The embedded frontend bundle is missing. Run <code>pnpm build</code> ",
    "(or <code>make build</code>) and restart the daemon. For HMR, set ",
    "<code>OZMUX_FRONTEND_DEV=1</code> on the daemon process and run ",
    "<code>make dev-frontend</code> in parallel.</p>\n",
);

fn main() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/handlers/index.html");
    if !path.exists() {
        fs::write(&path, STUB).expect("write index.html stub");
    }
    println!("cargo:rerun-if-changed=src/handlers/index.html");
}
