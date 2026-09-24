//! Embeds the orzma icon into the Windows executable as resource ordinal 1.

use embed_resource::{CompilationResult, NONE};

const ICON_RC: &str = "build/windows/orzma.rc";
const ICON_ICO: &str = "build/windows/orzma.ico";

fn main() -> Result<(), CompilationResult> {
    println!("cargo:rerun-if-changed={ICON_RC}");
    println!("cargo:rerun-if-changed={ICON_ICO}");
    embed_resource::compile(ICON_RC, NONE).manifest_required()
}
