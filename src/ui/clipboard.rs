//! Clipboard Bevy Resource. Wraps `arboard::Clipboard` so the rest of
//! the GUI can write text without holding the cross-platform handle
//! directly. Reads are not wired in MVP.

use bevy::ecs::resource::Resource;

/// Resource wrapping a lazily-initialized `arboard::Clipboard`.
///
/// `arboard::Clipboard::new()` can fail when no display is available
/// (e.g. headless CI). In that case `inner` stays `None` and every
/// `write` call becomes a no-op (logged at debug level once at init,
/// then silently dropped). Copy-mode UI keeps working — the user can
/// still see the selection — but `y` does not modify the host
/// clipboard.
#[derive(Resource)]
pub(crate) struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        match arboard::Clipboard::new() {
            Ok(cb) => Self { inner: Some(cb) },
            Err(e) => {
                tracing::warn!(
                    target: "ozmux_gui::clipboard",
                    error = ?e,
                    "arboard init failed; clipboard writes will no-op",
                );
                Self { inner: None }
            }
        }
    }

    /// Writes `text` to the system clipboard. No-op when arboard is
    /// unavailable. Failures are logged at warn but never propagated —
    /// copy mode must not panic on a clipboard failure.
    pub(crate) fn write(&mut self, text: String) {
        let Some(cb) = self.inner.as_mut() else {
            tracing::debug!(
                target: "ozmux_gui::clipboard",
                "clipboard write skipped: arboard unavailable",
            );
            return;
        };
        if let Err(e) = cb.set_text(text) {
            tracing::warn!(
                target: "ozmux_gui::clipboard",
                error = ?e,
                "clipboard write failed",
            );
        }
    }

    /// Reads text from the system clipboard. Returns `None` when
    /// `arboard` is unavailable (headless) or when the clipboard does
    /// not currently hold UTF-8 text. Empty strings are passed through
    /// as `Some(String::new())`; the caller is responsible for treating
    /// empty as a no-op.
    ///
    /// arboard's behavior on an empty clipboard is platform-dependent —
    /// some backends return `Err(ContentNotAvailable)`, others return
    /// `Ok("")`. Both shapes are handled here (the `Err` path returns
    /// `None`, the `Ok("")` path returns `Some("")`); either way the
    /// caller's `text.is_empty()` check at the dispatcher swallows it
    /// without reaching the PTY.
    pub(crate) fn read(&mut self) -> Option<String> {
        let Some(cb) = self.inner.as_mut() else {
            tracing::debug!(
                target: "ozmux_gui::clipboard",
                "clipboard read skipped: arboard unavailable",
            );
            return None;
        };
        match cb.get_text() {
            Ok(text) => Some(text),
            Err(arboard::Error::ContentNotAvailable) => {
                tracing::debug!(
                    target: "ozmux_gui::clipboard",
                    "clipboard read: nothing available (empty / non-text)",
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    target: "ozmux_gui::clipboard",
                    error = ?err,
                    "clipboard read failed",
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_returns_none_when_inner_is_unavailable() {
        // Force the unavailable-backend branch by constructing the
        // resource with `inner: None` directly. This mirrors what
        // `Clipboard::new` would do on a headless host where
        // `arboard::Clipboard::new()` fails.
        let mut cb = Clipboard { inner: None };
        assert!(cb.read().is_none(), "headless backend must yield None");
    }
}
