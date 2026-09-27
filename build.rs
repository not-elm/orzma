//! Embeds the orzma icon into the Windows executable as resource ordinal 1, and
//! links the Linux executable with an `$ORIGIN` RUNPATH so it loads the
//! `libcef.so` staged beside it.

use embed_resource::{CompilationResult, NONE};
use std::env;

const ICON_RC: &str = "build/windows/orzma.rc";
const ICON_ICO: &str = "build/windows/orzma.ico";
const LINUX_RPATH_LINK_ARG: &str = "-Wl,-rpath,$ORIGIN";

fn main() -> Result<(), CompilationResult> {
    println!("cargo:rerun-if-changed={ICON_RC}");
    println!("cargo:rerun-if-changed={ICON_ICO}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins={LINUX_RPATH_LINK_ARG}");
    }
    embed_resource::compile(ICON_RC, NONE).manifest_required()
}
