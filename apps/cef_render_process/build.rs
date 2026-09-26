//! Links the render process with an `$ORIGIN` RUNPATH on Linux so it loads the
//! `libcef.so` staged beside it.

use std::env;

const LINUX_RPATH_LINK_ARG: &str = "-Wl,-rpath,$ORIGIN";

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins={LINUX_RPATH_LINK_ARG}");
    }
}
