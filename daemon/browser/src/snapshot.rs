//! Latest-frame + latest-navigation snapshot shared via `tokio::sync::watch`.
//! Every produced frame is a full JPEG, so the snapshot keeps only the most
//! recent one — there is no delta/replay history (unlike the terminal VT).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Page navigation state captured from CDP events.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct NavState {
    /// Current document URL.
    pub url: String,
    /// Current document title.
    pub title: String,
}

/// Live viewport size in CSS pixels — distinct from the screencast frame's
/// pixel dimensions, which may be `viewport × DSF` for HiDPI displays.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
}

/// One JPEG screencast frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreencastFrame {
    /// Raw JPEG bytes as emitted by Chromium's `Page.startScreencast`.
    #[serde(with = "crate::bytes_serde")]
    pub jpeg: Bytes,
    /// Frame width in device pixels.
    pub width: u32,
    /// Frame height in device pixels.
    pub height: u32,
}

/// Combined latest-frame + navigation + viewport snapshot. Shared via
/// `tokio::sync::watch::channel<Arc<BrowserSnapshot>>` — a slow client
/// observes only the most recent value, intermediate frames are skipped.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrowserSnapshot {
    /// Latest screencast frame, if any.
    pub frame: Option<ScreencastFrame>,
    /// Latest navigation state.
    pub nav: NavState,
    /// Current Chromium viewport in CSS pixels. Zero until the first `Resize`
    /// message arrives from the frontend.
    pub viewport: Viewport,
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
            },
            viewport: Viewport {
                width: 1280,
                height: 800,
            },
        };
        let buf = rmp_serde::to_vec_named(&snap).unwrap();
        let back: BrowserSnapshot = rmp_serde::from_slice(&buf).unwrap();
        let frame = back.frame.expect("frame present");
        assert_eq!(frame.jpeg.as_ref(), &[0xFF, 0xD8, 0xFF, 0xD9]);
        assert_eq!(frame.width, 800);
        assert_eq!(back.nav.url, "https://example.com");
        assert_eq!(
            back.viewport,
            Viewport {
                width: 1280,
                height: 800
            }
        );
    }

    #[test]
    fn snapshot_default_has_no_frame_and_empty_nav_and_zero_viewport() {
        let s = BrowserSnapshot::default();
        assert!(s.frame.is_none());
        assert!(s.nav.url.is_empty());
        assert_eq!(s.viewport, Viewport::default());
    }

    #[test]
    fn viewport_round_trips_msgpack() {
        let vp = Viewport {
            width: 1920,
            height: 1080,
        };
        let buf = rmp_serde::to_vec_named(&vp).unwrap();
        let back: Viewport = rmp_serde::from_slice(&buf).unwrap();
        assert_eq!(back, vp);
    }
}
