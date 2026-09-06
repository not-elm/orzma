//! Test-only capture of `log` records, so a test can assert that a code path
//! raised no warning. Bevy's built-in command error handlers (the one behind
//! `EntityCommands::despawn`, for instance) report through `log::warn!`, not
//! through the app's fallback error handler, so this is the only way to see
//! them from a test.

use log::{Level, Log, Metadata, Record};
use std::sync::{Mutex, OnceLock};

static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INSTALL: OnceLock<()> = OnceLock::new();

struct CapturingLogger;

impl Log for CapturingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if record.level() <= Level::Warn {
            CAPTURED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// Installs the capturing logger once per test process and returns every
/// warning-or-worse message captured so far whose text contains `needle`.
///
/// Tests run concurrently in one process, so a caller compares the count
/// before and after the code under test rather than asserting emptiness.
pub(crate) fn warnings_containing(needle: &str) -> Vec<String> {
    INSTALL.get_or_init(|| {
        if log::set_logger(&CapturingLogger).is_ok() {
            log::set_max_level(log::LevelFilter::Warn);
        }
    });
    CAPTURED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|m| m.contains(needle))
        .cloned()
        .collect()
}
