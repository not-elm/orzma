//! Validation of a `register` payload into the content the host keeps, and
//! the mount spec and asset that content implies.

use crate::boundary::{ForwardChord, HandleId, MountSpec, WebviewAsset};
use crate::error::RegisterError;
use crate::protocol::RegisterKind;
use orzma_vt::prelude::PlacementSize;
use std::mem;
use std::path::{Path, PathBuf};
use url::Url;

/// A `register` payload that passed validation: a `dir` root is an absolute
/// directory and its entry a relative path of normal components, an
/// `inline` document is at most 4 MiB, and a `url` parses with an `http` or
/// `https` scheme and a host (kept in its normalized spelling).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRegistration {
    source: Source,
    entry: String,
    interactive: bool,
    forward_keys: Vec<ForwardChord>,
    preload: Vec<String>,
}

impl TryFrom<RegisterKind> for ValidatedRegistration {
    type Error = RegisterError;

    fn try_from(kind: RegisterKind) -> Result<Self, RegisterError> {
        match kind {
            RegisterKind::Dir {
                root,
                entry,
                interactive,
                forward_keys,
                preload,
            } => {
                let root = PathBuf::from(root);
                if !root.is_absolute() || !root.is_dir() {
                    return Err(RegisterError::InvalidRoot);
                }
                if !WebviewAsset::is_safe_relative_path(Path::new(&entry)) {
                    return Err(RegisterError::UnsafeEntry);
                }
                Ok(Self {
                    source: Source::Dir(root),
                    entry,
                    interactive,
                    forward_keys,
                    preload,
                })
            }
            RegisterKind::Inline {
                html,
                interactive,
                forward_keys,
                preload,
            } => {
                if html.len() > Self::MAX_INLINE_HTML {
                    return Err(RegisterError::HtmlTooLarge);
                }
                Ok(Self {
                    source: Source::Inline(html),
                    entry: "index.html".into(),
                    interactive,
                    forward_keys,
                    preload,
                })
            }
            RegisterKind::Url {
                url,
                interactive,
                bridge,
                forward_keys,
                preload,
            } => Ok(Self {
                source: Source::Url {
                    url: validate_url(&url)?,
                    bridge,
                },
                entry: String::new(),
                interactive,
                forward_keys,
                preload,
            }),
        }
    }
}

impl ValidatedRegistration {
    /// Upper bound on a single inline HTML document (4 MiB).
    const MAX_INLINE_HTML: usize = 4 * 1024 * 1024;

    /// The spec a mount of this content under `handle`, over a rect of
    /// `size` cells, needs.
    pub(crate) fn mount_spec(&self, handle: &HandleId, size: PlacementSize) -> MountSpec {
        let url = match &self.source {
            Source::Dir(_) => format!("orzma://{handle}/{}", self.entry),
            Source::Inline(_) => format!("orzma://{handle}/index.html"),
            Source::Url { url, .. } => url.clone(),
        };
        MountSpec::new(handle.clone(), url, size)
            .with_interactive(self.interactive)
            .with_bridge(self.source.is_bridged())
            .with_preload(self.preload.clone())
            .with_forward_keys(self.forward_keys.clone())
    }

    /// Takes the asset the GUI serves for this content: a directory root or
    /// an inline document, and `None` for a remote URL. An inline document's
    /// bytes move into the asset, so a later call returns an empty document.
    pub(crate) fn take_asset(&mut self) -> Option<WebviewAsset> {
        match &mut self.source {
            Source::Dir(root) => Some(WebviewAsset::Dir(root.clone())),
            Source::Inline(html) => Some(WebviewAsset::Inline(mem::take(html).into_bytes())),
            Source::Url { .. } => None,
        }
    }

    /// Whether the GUI serves an asset for this content.
    pub(crate) fn serves_asset(&self) -> bool {
        !matches!(self.source, Source::Url { .. })
    }

    /// Whether a page of this content accepts pointer and keyboard input.
    pub(crate) fn interactive(&self) -> bool {
        self.interactive
    }

    /// Whether a page of this content gets the `window.orzma` bridge.
    pub(crate) fn is_bridged(&self) -> bool {
        self.source.is_bridged()
    }

    /// Whether this content is a remote `http(s)` URL.
    pub(crate) fn is_url(&self) -> bool {
        matches!(self.source, Source::Url { .. })
    }

    /// Replaces the forward-key chords.
    pub(crate) fn set_forward_keys(&mut self, keys: Vec<ForwardChord>) {
        self.forward_keys = keys;
    }
}

/// Validates a URL a program asked to load: parses it, requires an `http`
/// or `https` scheme, then a non-empty host, and returns the
/// parser-normalized URL. A non-`http(s)` scheme reports
/// [`RegisterError::UnsupportedScheme`] even when the URL also has no host.
pub(crate) fn validate_url(url: &str) -> Result<String, RegisterError> {
    let parsed = Url::parse(url).map_err(|_| RegisterError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(RegisterError::UnsupportedScheme);
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(RegisterError::InvalidUrl);
    }
    Ok(parsed.into())
}

/// Where a registration's content lives.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// Files under this absolute root, served through `orzma://`.
    Dir(PathBuf),
    /// One inline HTML document, served through `orzma://`.
    Inline(String),
    /// A remote `http(s)` URL CEF loads directly.
    Url {
        /// The normalized URL.
        url: String,
        /// Whether the page gets the `window.orzma` bridge.
        bridge: bool,
    },
}

impl Source {
    /// Whether a page of this source gets the `window.orzma` bridge: always
    /// for `dir` and `inline`, and only on request for `url`.
    fn is_bridged(&self) -> bool {
        match self {
            Self::Dir(_) | Self::Inline(_) => true,
            Self::Url { bridge, .. } => *bridge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: PlacementSize = PlacementSize { rows: 10, cols: 40 };

    fn dir(root: &str, entry: &str) -> RegisterKind {
        RegisterKind::Dir {
            root: root.into(),
            entry: entry.into(),
            interactive: true,
            forward_keys: vec![],
            preload: vec![],
        }
    }

    fn url(url: &str, bridge: bool) -> RegisterKind {
        RegisterKind::Url {
            url: url.into(),
            interactive: true,
            bridge,
            forward_keys: vec![],
            preload: vec![],
        }
    }

    /// Asserts that a `dir` root must be an existing absolute directory and
    /// its entry a relative path of normal components.
    ///
    /// Case: programs register a relative root, a missing root, a traversal
    /// entry, and a correct bundle directory.
    #[test]
    fn a_dir_needs_an_absolute_root_and_a_safe_entry() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_str().unwrap().to_string();
        assert_eq!(
            ValidatedRegistration::try_from(dir("relative/ui", "index.html")),
            Err(RegisterError::InvalidRoot)
        );
        assert_eq!(
            ValidatedRegistration::try_from(dir(&format!("{root}/missing"), "index.html")),
            Err(RegisterError::InvalidRoot)
        );
        assert_eq!(
            ValidatedRegistration::try_from(dir(&root, "../escape.html")),
            Err(RegisterError::UnsafeEntry)
        );
        assert_eq!(
            ValidatedRegistration::try_from(dir(&root, "")),
            Err(RegisterError::UnsafeEntry)
        );
        assert!(ValidatedRegistration::try_from(dir(&root, "app/index.html")).is_ok());
    }

    /// Asserts that an inline document of exactly 4 MiB is accepted and one
    /// byte more is refused.
    ///
    /// Case: a program inlines a large generated report.
    #[test]
    fn an_inline_document_may_be_at_most_4_mib() {
        let inline = |len: usize| RegisterKind::Inline {
            html: "x".repeat(len),
            interactive: true,
            forward_keys: vec![],
            preload: vec![],
        };
        assert!(
            ValidatedRegistration::try_from(inline(ValidatedRegistration::MAX_INLINE_HTML)).is_ok()
        );
        assert_eq!(
            ValidatedRegistration::try_from(inline(ValidatedRegistration::MAX_INLINE_HTML + 1)),
            Err(RegisterError::HtmlTooLarge)
        );
    }

    /// Asserts that a URL must be `http` or `https` with a host, and that a
    /// non-web scheme is reported as unsupported rather than invalid.
    ///
    /// Case: programs register a remote page, a local file, a script URL,
    /// garbage, and an `http` URL with no host.
    #[test]
    fn a_url_must_be_web_with_a_host() {
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("https://example.com/a?b=c").is_ok());
        assert_eq!(
            validate_url("file:///etc/passwd"),
            Err(RegisterError::UnsupportedScheme)
        );
        assert_eq!(
            validate_url("javascript:alert(1)"),
            Err(RegisterError::UnsupportedScheme)
        );
        assert_eq!(validate_url("not a url"), Err(RegisterError::InvalidUrl));
        assert_eq!(validate_url("http://"), Err(RegisterError::InvalidUrl));
    }

    /// Asserts that a registered URL is kept in its normalized spelling.
    ///
    /// Case: a program registers a URL typed with an uppercase scheme and
    /// host and no path.
    #[test]
    fn a_url_is_normalized() {
        assert_eq!(
            validate_url("HTTPS://Example.com").as_deref(),
            Ok("https://example.com/")
        );
    }

    /// Asserts that each source's mount spec loads the URL that source
    /// serves, and carries the input policy, bridge, preload, and chords.
    ///
    /// Case: one program registers a bundle directory, one an inline
    /// document, and one a read-only remote page with a preload script and
    /// a forward key.
    #[test]
    fn a_mount_spec_follows_the_registration() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_str().unwrap().to_string();
        let handle = HandleId::from("h");
        let dir_spec = ValidatedRegistration::try_from(dir(&root, "app/index.html"))
            .unwrap()
            .mount_spec(&handle, SIZE);
        assert_eq!(dir_spec.url(), "orzma://h/app/index.html");
        assert!(dir_spec.bridged());
        let inline_spec = ValidatedRegistration::try_from(RegisterKind::Inline {
            html: "<p>x</p>".into(),
            interactive: false,
            forward_keys: vec![],
            preload: vec![],
        })
        .unwrap()
        .mount_spec(&handle, SIZE);
        assert_eq!(inline_spec.url(), "orzma://h/index.html");
        assert!(!inline_spec.interactive());
        let chord = ForwardChord::new(vec!["alt".into()], "h");
        let url_spec = ValidatedRegistration::try_from(RegisterKind::Url {
            url: "https://example.com/".into(),
            interactive: true,
            bridge: false,
            forward_keys: vec![chord.clone()],
            preload: vec!["window.A = 1;".into()],
        })
        .unwrap()
        .mount_spec(&handle, SIZE);
        assert_eq!(url_spec.url(), "https://example.com/");
        assert!(!url_spec.bridged());
        assert_eq!(url_spec.preload(), ["window.A = 1;".to_string()]);
        assert_eq!(url_spec.forward_keys(), [chord]);
        assert_eq!(url_spec.size(), SIZE);
    }

    /// Asserts that a directory and an inline document are served as
    /// assets, and a remote URL is not.
    ///
    /// Case: the GUI's `orzma://` scheme learns what to serve for each new
    /// registration.
    #[test]
    fn only_dir_and_inline_content_is_served_as_an_asset() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        let mut dir_content =
            ValidatedRegistration::try_from(dir(root_path.to_str().unwrap(), "index.html"))
                .unwrap();
        assert_eq!(dir_content.take_asset(), Some(WebviewAsset::Dir(root_path)));
        assert!(dir_content.serves_asset());
        let mut url_content =
            ValidatedRegistration::try_from(url("https://example.com", true)).unwrap();
        assert_eq!(url_content.take_asset(), None);
        assert!(!url_content.serves_asset());
    }
}
