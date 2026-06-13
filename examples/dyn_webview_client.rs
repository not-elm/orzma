//! Minimal reference client for ozmux Tier 1 dynamic webviews. Run inside an
//! ozmux pane: it reads `$OZMUX_SOCK`/`$OZMUX_TOKEN`, registers an inline HTML
//! view over the control socket, prints the `mount-inline` OSC at the cursor,
//! then stays alive so the registration persists.
//!
//! Usage (inside an ozmux pane, with the control plane up):
//!   cargo run --example dyn_webview_client

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let sock = std::env::var("OZMUX_SOCK")
        .expect("not inside an ozmux pane (or control plane down): $OZMUX_SOCK unset");
    let token = std::env::var("OZMUX_TOKEN").expect("$OZMUX_TOKEN unset");

    let mut stream = UnixStream::connect(&sock)?;
    let mut reader = BufReader::new(stream.try_clone()?);

    writeln!(stream, "{}", json!({ "op": "hello", "token": token }))?;
    let html = "<body style='background:#13131a;color:#8be9fd;font:16px sans-serif;margin:0;padding:8px'>\
                <h1>hello from a TUI app</h1><p>rendered inline by ozmux</p></body>";
    writeln!(
        stream,
        "{}",
        json!({ "op": "register", "kind": "inline", "html": html, "interactive": false })
    )?;
    stream.flush()?;

    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply: Value =
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"));
    let handle = reply
        .get("handle")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("register failed: {reply}"));

    let rows = 8;
    let cols = 48;
    print!("dynamic webview:\n\x1b]5379;mount-inline;{handle};{rows};{cols}\x1b\\");
    for _ in 0..rows {
        println!();
    }
    std::io::stdout().flush()?;

    // NOTE: the registration lives only as long as this connection; exiting (or closing the socket) tears the inline webview down.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
