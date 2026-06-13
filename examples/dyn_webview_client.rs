//! Minimal reference client for ozmux Tier 1 dynamic webviews. Run inside an
//! ozmux pane: it reads `$OZMUX_SOCK`/`$OZMUX_TOKEN`, registers an inline HTML
//! view over the control socket, prints the `mount-inline` OSC at the cursor,
//! then demonstrates the back-channel by:
//!   - replying to `ping` calls from the page (`window.ozmux.call`)
//!   - emitting a `tick` event every second (`window.ozmux.on`)
//!
//! The page also has an `<input>`, so clicking it shows the focus ring and typing
//! (routed to the focused inline webview) echoes back into the page.
//!
//! Usage (inside an ozmux pane, with the control plane up):
//!   cargo run --example dyn_webview_client

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let sock = std::env::var("OZMUX_SOCK")
        .expect("not inside an ozmux pane (or control plane down): $OZMUX_SOCK unset");
    let token = std::env::var("OZMUX_TOKEN").expect("$OZMUX_TOKEN unset");

    let stream = UnixStream::connect(&sock)?;
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    let mut reader = BufReader::new(stream);

    {
        let mut w = writer.lock().unwrap();
        writeln!(w, "{}", json!({ "op": "hello", "token": token }))?;
        let html = concat!(
            "<body style='background:#13131a;color:#8be9fd;font:16px sans-serif;margin:0;padding:8px'>",
            "<h1>window.ozmux demo</h1>",
            "<div id='out'>calling ping\u{2026}</div>",
            "<div id='tick'>no ticks yet</div>",
            // A focusable element so click-to-focus is visible: clicking it shows the
            // browser focus ring, and typing (routed to the focused webview) echoes here.
            "<input id='field' placeholder='click here, then type\u{2026}' ",
            "style='font:16px sans-serif;padding:4px;width:92%;margin-top:8px'>",
            "<script>",
            "window.ozmux.call('ping',['hi'])",
            ".then(function(v){document.getElementById('out').textContent='ping \u{2192} '+v;})",
            ".catch(function(e){document.getElementById('out').textContent='error: '+e.message;});",
            "window.ozmux.on('tick',function(n){document.getElementById('tick').textContent='tick #'+n;});",
            "document.getElementById('field').addEventListener('input',function(e){",
            "document.getElementById('out').textContent='typed: '+e.target.value;});",
            "</script>",
            "</body>"
        );
        writeln!(
            w,
            "{}",
            json!({ "op": "register", "kind": "inline", "html": html, "interactive": true })
        )?;
        w.flush()?;
    }

    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply: Value =
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"));
    let handle = reply
        .get("handle")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("register failed: {reply}"))
        .to_owned();

    let rows = 8u16;
    let cols = 48u16;
    print!("dynamic webview:\n\x1b]5379;mount-inline;{handle};{rows};{cols}\x1b\\");
    for _ in 0..rows {
        println!();
    }
    std::io::stdout().flush()?;

    // Tick emitter: sends {op:"emit", handle, event:"tick", payload: n} each second.
    {
        let tick_handle = handle.clone();
        let tick_writer = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut n: u64 = 0;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                n += 1;
                let msg =
                    json!({ "op": "emit", "handle": tick_handle, "event": "tick", "payload": n });
                let mut w = tick_writer.lock().unwrap();
                // NOTE: ignore write errors on the emitter thread; the main thread will detect EOF and exit.
                let _ = writeln!(w, "{msg}");
                let _ = w.flush();
            }
        });
    }

    // Back-channel serve loop: dispatch incoming {op:"call"} messages.
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        }
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if msg.get("op").and_then(Value::as_str) != Some("call") {
            continue;
        }
        let Some(req_id) = msg.get("reqId") else {
            continue;
        };
        let req_id = req_id.clone();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let reply = if method == "ping" {
            let arg = msg
                .get("args")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({ "op": "reply", "reqId": req_id, "ok": true, "value": format!("pong:{arg}") })
        } else {
            json!({ "op": "reply", "reqId": req_id, "ok": false, "error": "unknown_method" })
        };
        let mut w = writer.lock().unwrap();
        if writeln!(w, "{reply}").is_err() {
            break;
        }
        let _ = w.flush();
    }

    Ok(())
}
