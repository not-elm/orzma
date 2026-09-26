//! The plain-data vocabulary the webview host and the GUI exchange: what
//! the host tells the GUI to show, and what the GUI reports back.

use crate::error::WebviewHostResult;
use crate::host::mint::random_base32;
use orzma_vt::prelude::{InstanceId, PlacementSize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::PathBuf;

/// The content backing one dynamic handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebviewAsset {
    /// Files served under this absolute root directory.
    Dir(PathBuf),
    /// A single inline HTML document served from memory.
    Inline(Vec<u8>),
}

/// The opaque identity of one dynamic registration.
///
/// It is the host of that registration's `orzma://<handle>/` origin, the
/// routing key for its back-channel, and the ownership unit `unregister`
/// and connection teardown act on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandleId(String);

impl HandleId {
    /// Borrows the wire spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A fresh handle: 128 random bits in lowercase unpadded base32,
    /// usable verbatim as an `orzma://` host.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewHostError::Csprng`](crate::error::WebviewHostError::Csprng)
    /// when the OS random source fails.
    pub(crate) fn mint() -> WebviewHostResult<Self> {
        random_base32().map(Self)
    }
}

// NOTE: the inbound direction is safe and needed (the wire hands us
// strings); it is the outbound `From<HandleId> for String` that must not
// exist, or a handle flows into an `impl Into<String>` slot wanting an
// instance id.
impl From<&str> for HandleId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for HandleId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One mount of a placement.
///
/// The host mints a fresh value each time an unmounted placement is
/// mounted, and never reuses one while it runs; mounting an already mounted
/// placement again keeps its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MountId(u64);

impl MountId {
    /// The mount spelled `raw`.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The mount minted after this one.
    pub(crate) fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// A forward-key chord in its wire form: modifier names and a key name.
///
/// The host keeps it uninterpreted. The GUI matches it against key presses
/// and ignores a chord whose names it does not recognize.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForwardChord {
    mods: Vec<String>,
    key: String,
}

impl ForwardChord {
    /// The chord of `key` with the modifiers named in `mods` (`alt`,
    /// `ctrl`, `shift`, `meta`).
    pub fn new(mods: Vec<String>, key: impl Into<String>) -> Self {
        Self {
            mods,
            key: key.into(),
        }
    }

    /// The modifier names.
    pub fn mods(&self) -> &[String] {
        &self.mods
    }

    /// The key name: a lowercase letter or digit, a named key such as `tab`
    /// or `f1`, or one ASCII punctuation character.
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// Everything the GUI needs to create the webview of one mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    handle: HandleId,
    url: String,
    interactive: bool,
    bridged: bool,
    preload: Vec<String>,
    forward_keys: Vec<ForwardChord>,
    size: PlacementSize,
}

impl MountSpec {
    /// A spec that loads `url` for `handle` over a rect of `size` cells:
    /// interactive, without the `window.orzma` bridge, with no preload
    /// scripts and no forward keys.
    pub fn new(handle: HandleId, url: impl Into<String>, size: PlacementSize) -> Self {
        Self {
            handle,
            url: url.into(),
            interactive: true,
            bridged: false,
            preload: Vec::new(),
            forward_keys: Vec::new(),
            size,
        }
    }

    /// This spec, accepting pointer and keyboard input only when
    /// `interactive`.
    pub fn with_interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// This spec, injecting the `window.orzma` bridge only when `bridged`.
    pub fn with_bridge(mut self, bridged: bool) -> Self {
        self.bridged = bridged;
        self
    }

    /// This spec with `preload` as the registering program's scripts.
    pub fn with_preload(mut self, preload: Vec<String>) -> Self {
        self.preload = preload;
        self
    }

    /// This spec with `forward_keys` as the chords that bypass the page.
    pub fn with_forward_keys(mut self, forward_keys: Vec<ForwardChord>) -> Self {
        self.forward_keys = forward_keys;
        self
    }

    /// The registration the content comes from.
    pub fn handle(&self) -> &HandleId {
        &self.handle
    }

    /// The URL to load: `orzma://<handle>/<entry>` for a directory or inline
    /// registration, the remote URL for a `url` one.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether the page receives pointer and keyboard input.
    pub fn interactive(&self) -> bool {
        self.interactive
    }

    /// Whether the page gets the `window.orzma` bridge.
    pub fn bridged(&self) -> bool {
        self.bridged
    }

    /// The registering program's preload scripts, in order.
    pub fn preload(&self) -> &[String] {
        &self.preload
    }

    /// The chords that reach the pane's PTY instead of the page.
    pub fn forward_keys(&self) -> &[ForwardChord] {
        &self.forward_keys
    }

    /// The rect's extent in cells.
    pub fn size(&self) -> PlacementSize {
        self.size
    }
}

/// A navigation of one mount's webview that the host validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    /// Load this `http(s)` URL, already normalized by the host.
    To(String),
    /// Go back in the webview's session history.
    Back,
    /// Go forward in the webview's session history.
    Forward,
    /// Reload the current page.
    Reload,
}

/// How a page's `window.orzma.call` settles: `Ok` with the program's value,
/// or `Err` with an error string.
pub type PageOutcome = Result<Value, String>;

/// What the host tells the GUI. `P` names the pane a mount belongs to.
#[derive(Debug, Clone, PartialEq)]
pub enum WebviewEvent<P> {
    /// The assets of `handle` are now served from `asset`.
    AssetRegistered {
        /// The registration whose assets are served.
        handle: HandleId,
        /// Where the assets come from.
        asset: WebviewAsset,
    },
    /// The assets of `handle` are no longer served.
    AssetReleased {
        /// The released registration.
        handle: HandleId,
    },
    /// A placement became mounted in `pane`; the GUI creates its webview.
    Mounted {
        /// The pane the placement sits in.
        pane: P,
        /// The new mount.
        mount: MountId,
        /// The placement the mount belongs to.
        instance: InstanceId,
        /// How to build the webview.
        spec: MountSpec,
    },
    /// A mounted placement's rect changed size.
    Resized {
        /// The mount whose rect changed.
        mount: MountId,
        /// The new extent in cells.
        size: PlacementSize,
    },
    /// These mounts ended; the GUI removes their webviews.
    Unmounted {
        /// The mounts that ended.
        mounts: Vec<MountId>,
    },
    /// The mount whose webview holds keyboard focus, or `None` when no
    /// webview holds it.
    FocusChanged {
        /// The focused mount.
        focused: Option<MountId>,
    },
    /// The forward-key chords of every mount of `handle` are now `keys`.
    ForwardKeysChanged {
        /// The registration whose chords changed.
        handle: HandleId,
        /// The complete new chord list.
        keys: Vec<ForwardChord>,
    },
    /// Settles the call `page_req` made by the page of `mount`.
    PageReply {
        /// The mount whose page made the call.
        mount: MountId,
        /// The page's own id for the call.
        page_req: String,
        /// How the call settles.
        outcome: PageOutcome,
    },
    /// Delivers a program's `emit` to the page of `mount`.
    PageEvent {
        /// The mount whose page receives the event.
        mount: MountId,
        /// The event name.
        event: String,
        /// The event payload.
        payload: Value,
    },
    /// Navigates the webview of `mount`.
    Navigate {
        /// The mount to navigate.
        mount: MountId,
        /// What to do.
        navigation: Navigation,
    },
}

impl<P> WebviewEvent<P> {
    /// This event with its pane replaced through `resolve`, or `None` when
    /// the event names a pane `resolve` returns `None` for. Only
    /// [`WebviewEvent::Mounted`] names a pane; every other event passes
    /// through unchanged.
    pub fn try_map_pane<Q>(self, resolve: impl FnOnce(P) -> Option<Q>) -> Option<WebviewEvent<Q>> {
        Some(match self {
            Self::AssetRegistered { handle, asset } => {
                WebviewEvent::AssetRegistered { handle, asset }
            }
            Self::AssetReleased { handle } => WebviewEvent::AssetReleased { handle },
            Self::Mounted {
                pane,
                mount,
                instance,
                spec,
            } => WebviewEvent::Mounted {
                pane: resolve(pane)?,
                mount,
                instance,
                spec,
            },
            Self::Resized { mount, size } => WebviewEvent::Resized { mount, size },
            Self::Unmounted { mounts } => WebviewEvent::Unmounted { mounts },
            Self::FocusChanged { focused } => WebviewEvent::FocusChanged { focused },
            Self::ForwardKeysChanged { handle, keys } => {
                WebviewEvent::ForwardKeysChanged { handle, keys }
            }
            Self::PageReply {
                mount,
                page_req,
                outcome,
            } => WebviewEvent::PageReply {
                mount,
                page_req,
                outcome,
            },
            Self::PageEvent {
                mount,
                event,
                payload,
            } => WebviewEvent::PageEvent {
                mount,
                event,
                payload,
            },
            Self::Navigate { mount, navigation } => WebviewEvent::Navigate { mount, navigation },
        })
    }
}

/// What the GUI reports to the host.
#[derive(Debug, Clone, PartialEq)]
pub enum WebviewCommand {
    /// The user focused the webview of `mount` (`Some`), or released
    /// webview focus (`None`) with an off-rect click, the release-focus
    /// shortcut, or vi mode. The host answers with a `FocusChanged`.
    Focus {
        /// The mount the user focused.
        mount: Option<MountId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn size() -> PlacementSize {
        PlacementSize { rows: 10, cols: 40 }
    }

    /// Asserts that a handle spells as the string it was built from, both
    /// through `as_str` and `Display`.
    ///
    /// Case: the host builds an `orzma://<handle>/` origin from a minted
    /// handle.
    #[test]
    fn a_handle_spells_as_its_wire_string() {
        let handle = HandleId::from("abc2");
        assert_eq!(handle.as_str(), "abc2");
        assert_eq!(handle.to_string(), "abc2");
    }

    /// Asserts that a chord deserializes from its wire form into its
    /// modifier names and key name.
    ///
    /// Case: a TUI browser registers Alt+H as a forward key.
    #[test]
    fn a_forward_chord_deserializes_from_its_wire_form() {
        let chord: ForwardChord =
            serde_json::from_str(r#"{"mods":["alt"],"key":"h"}"#).expect("a valid chord");
        assert_eq!(chord.mods(), ["alt".to_string()]);
        assert_eq!(chord.key(), "h");
        assert_eq!(chord, ForwardChord::new(vec!["alt".into()], "h"));
    }

    /// Asserts that a new spec is interactive and unbridged with no scripts
    /// or chords, and that each builder method overrides one field.
    ///
    /// Case: the host builds the spec for a bridged, non-interactive inline
    /// view with a preload script and a forward key.
    #[test]
    fn a_mount_spec_defaults_and_overrides() {
        let spec = MountSpec::new(HandleId::from("h"), "orzma://h/index.html", size());
        assert!(spec.interactive());
        assert!(!spec.bridged());
        assert!(spec.preload().is_empty());
        assert!(spec.forward_keys().is_empty());
        let chord = ForwardChord::new(vec![], "esc");
        let spec = spec
            .with_interactive(false)
            .with_bridge(true)
            .with_preload(vec!["window.A = 1;".into()])
            .with_forward_keys(vec![chord.clone()]);
        assert_eq!(spec.handle(), &HandleId::from("h"));
        assert_eq!(spec.url(), "orzma://h/index.html");
        assert!(!spec.interactive());
        assert!(spec.bridged());
        assert_eq!(spec.preload(), ["window.A = 1;".to_string()]);
        assert_eq!(spec.forward_keys(), [chord]);
        assert_eq!(spec.size(), size());
    }

    /// Asserts that `try_map_pane` resolves the pane of a mount, and drops
    /// the mount when its pane does not resolve.
    ///
    /// Case: the GUI bridge maps a backend pane to its entity, once for a
    /// live pane and once for a pane that closed in the same drain.
    #[test]
    fn try_map_pane_resolves_or_drops_a_mount() {
        let mounted = || WebviewEvent::Mounted {
            pane: 7_u32,
            mount: MountId::new(1),
            instance: InstanceId(9),
            spec: MountSpec::new(HandleId::from("h"), "orzma://h/index.html", size()),
        };
        let resolved = mounted().try_map_pane(|pane| Some(pane * 10));
        assert!(matches!(
            resolved,
            Some(WebviewEvent::Mounted { pane: 70, .. })
        ));
        assert_eq!(mounted().try_map_pane(|_| None::<u32>), None);
    }

    /// Asserts that an event naming no pane passes through `try_map_pane`
    /// even when no pane resolves.
    ///
    /// Case: the GUI bridge receives a focus change and a page reply after
    /// the pane registry emptied.
    #[test]
    fn try_map_pane_passes_every_other_event_through() {
        let focus = WebviewEvent::<u32>::FocusChanged {
            focused: Some(MountId::new(3)),
        };
        assert_eq!(
            focus.try_map_pane(|_| None::<u32>),
            Some(WebviewEvent::FocusChanged {
                focused: Some(MountId::new(3))
            })
        );
        let reply = WebviewEvent::<u32>::PageReply {
            mount: MountId::new(3),
            page_req: "p0".into(),
            outcome: Ok(json!(1)),
        };
        assert_eq!(
            reply.clone().try_map_pane(|_| None::<u32>),
            Some(WebviewEvent::PageReply {
                mount: MountId::new(3),
                page_req: "p0".into(),
                outcome: Ok(json!(1)),
            })
        );
    }
}
