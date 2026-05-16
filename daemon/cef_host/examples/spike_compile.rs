//! Verifies that cef + cef-dll-sys 148 builds with sandbox feature on the host.
//! Output is a findings note: build success/failure + any binding gotchas.

use cef::{App, MainArgs};

fn main() {
    println!("cef-rs 148 compile smoke test");
    let _args = MainArgs::default();
    println!("MainArgs::default() OK");
    let _app: Option<App> = None;
    println!("App typed-Option OK — binding is reachable");
}
