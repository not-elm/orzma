//! Embeds the built web bundle and materializes it to a temp directory at
//! runtime so it can be served through `Webview::dir`.

use rust_embed::RustEmbed;
use std::fs;
use tempfile::TempDir;

/// The built web bundle, embedded at compile time from `assets/`.
#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

/// Writes every embedded asset into a fresh temp directory and returns it.
///
/// Keep the returned [`TempDir`] alive for the lifetime of the app; dropping it
/// deletes the materialized files.
///
/// # Errors
/// Returns an error if the temp dir or any file cannot be written.
pub(crate) fn materialize() -> std::io::Result<TempDir> {
    let dir = tempfile::tempdir()?;
    for name in Assets::iter() {
        let file = Assets::get(name.as_ref()).expect("embedded asset listed by iter() must exist");
        let dest = dir.path().join(name.as_ref());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, file.data.as_ref())?;
    }
    Ok(dir)
}
