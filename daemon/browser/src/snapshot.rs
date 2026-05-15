//! Latest-frame + latest-navigation snapshot shared via `tokio::sync::watch`.
//! Every produced frame is a full JPEG, so the snapshot keeps only the most
//! recent one — there is no delta/replay history (unlike the terminal VT).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Serde helper: serialize/deserialize `bytes::Bytes` as a compact msgpack
/// binary using `serde_bytes`, which emits a bin format cell instead of a
/// sequence of integers.
mod bytes_serde {
    use bytes::Bytes;
    use serde::{Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(v: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::serialize(v.as_ref(), s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let buf: Vec<u8> = serde_bytes::deserialize(d)?;
        Ok(Bytes::from(buf))
    }
}

/// Page navigation state captured from CDP events.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NavState {
    /// Current document URL.
    pub url: String,
    /// Current document title.
    pub title: String,
    /// Whether a navigation is in flight.
    pub loading: bool,
    /// Whether the browser can navigate back.
    pub can_go_back: bool,
    /// Whether the browser can navigate forward.
    pub can_go_forward: bool,
}

/// One JPEG screencast frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreencastFrame {
    /// Raw JPEG bytes as emitted by Chromium's `Page.startScreencast`.
    #[serde(with = "bytes_serde")]
    pub jpeg: Bytes,
    /// Frame width in device pixels.
    pub width: u32,
    /// Frame height in device pixels.
    pub height: u32,
}

/// Combined latest-frame + navigation snapshot. Shared via
/// `tokio::sync::watch::channel<Arc<BrowserSnapshot>>` — a slow client
/// observes only the most recent value, intermediate frames are skipped.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrowserSnapshot {
    /// Latest screencast frame, if any.
    pub frame: Option<ScreencastFrame>,
    /// Latest navigation state.
    pub nav: NavState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_msgpack() {
        let snap = BrowserSnapshot {
            frame: Some(ScreencastFrame {
                jpeg: bytes::Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9]),
                width: 800,
                height: 600,
            }),
            nav: NavState {
                url: "https://example.com".into(),
                title: "Example".into(),
                loading: false,
                can_go_back: false,
                can_go_forward: false,
            },
        };
        let buf = rmp_serde::to_vec_named(&snap).unwrap();
        let back: BrowserSnapshot = rmp_serde::from_slice(&buf).unwrap();
        let frame = back.frame.expect("frame present");
        assert_eq!(frame.jpeg.as_ref(), &[0xFF, 0xD8, 0xFF, 0xD9]);
        assert_eq!(frame.width, 800);
        assert_eq!(back.nav.url, "https://example.com");
    }

    #[test]
    fn snapshot_default_has_no_frame_and_empty_nav() {
        let s = BrowserSnapshot::default();
        assert!(s.frame.is_none());
        assert!(s.nav.url.is_empty());
        assert!(!s.nav.loading);
    }
}
