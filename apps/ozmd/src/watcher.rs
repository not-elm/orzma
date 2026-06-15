//! Debounced file watcher. Watches the target file's parent directory (so
//! atomic save = write-temp-then-rename is caught) and sends a reload signal on
//! the channel whenever an event references the target path.

use notify_debouncer_full::notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// A live watcher; dropping it stops the background thread.
pub(crate) struct FileWatcher {
    _debouncer: Debouncer<notify_debouncer_full::notify::RecommendedWatcher, FileIdMap>,
}

/// Starts watching `path` (via its parent directory). Sends `()` on `tx` after a
/// debounced settle whenever an event references `path`.
///
/// # Errors
/// Returns an error if the watcher cannot be created or the parent cannot be watched.
pub(crate) fn watch(path: &Path, tx: Sender<()>) -> anyhow::Result<FileWatcher> {
    let target: PathBuf = path.to_path_buf();
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        None,
        move |res: DebounceEventResult| {
            if let Ok(events) = res
                && events.iter().any(|e| e.paths.iter().any(|p| p == &target))
            {
                let _ = tx.send(());
            }
        },
    )?;
    debouncer
        .watcher()
        .watch(&parent, RecursiveMode::NonRecursive)?;
    debouncer
        .cache()
        .add_root(&parent, RecursiveMode::NonRecursive);
    Ok(FileWatcher {
        _debouncer: debouncer,
    })
}
