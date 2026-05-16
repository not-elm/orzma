//! Wire schemas for daemon ↔ cef_host (HostCommand/HostEvent) and
//! daemon ↔ frontend (BrowserClientMsg/BrowserServerMsg).
//!
//! PoC subset: not all variants needed for full feature parity are included.
//! See spec doc Section 15 for the complete v3/v4 wire schema. Plan 2 will
//! add the missing variants (Nav, Input, IME, Copy/Paste, etc.).

use crate::types::{ActivityId, FrameKey, Rect};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// SameSite policy for a cookie transferred to cef_host at BrowserCreate time.
/// Task A5 reads these from the `decrypt-cookies` crate output and forwards
/// them so cef_host can seed the embedded browser's cookie store.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameSite {
    /// Strict SameSite policy.
    Strict,
    /// Lax SameSite policy.
    Lax,
    /// No SameSite restriction.
    None,
    /// SameSite not specified by the origin server.
    Unspecified,
}

/// A single cookie entry forwarded to cef_host via BrowserCreate ancillary
/// data. Placeholder for Task A5; the full plumbing (SCM_RIGHTS + cookie
/// bootstrap in cef_host) is wired there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CefCookieDto {
    /// The URL the cookie is scoped to.
    pub url: String,
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain attribute.
    pub domain: String,
    /// Path attribute.
    pub path: String,
    /// Secure flag.
    pub secure: bool,
    /// HttpOnly flag.
    pub http_only: bool,
    /// Expiry as Windows FILETIME microseconds, or `None` for session cookies.
    pub expires_utc: Option<f64>,
    /// SameSite policy.
    pub same_site: SameSite,
}

/// daemon → cef_host. PoC subset.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostCommand {
    /// Initial daemon → cef_host configuration after Hello handshake.
    Ready {
        runtime_root: String,
    },
    /// Create a new BrowserActivity. The shm fd is passed out-of-band via SCM_RIGHTS;
    /// cookies are forwarded inline so cef_host can seed the cookie store (Task B12).
    BrowserCreate {
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        cookies: Vec<CefCookieDto>,
    },
    Resize {
        aid: ActivityId,
        css_w: u32,
        css_h: u32,
        dpr: f32,
    },
    Close {
        aid: ActivityId,
    },
    Shutdown,
}

/// cef_host → daemon. PoC subset.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostEvent {
    /// Initial handshake greeting from cef_host.
    Hello {
        cef_version: String,
        abi_version: u32,
        pid: u32,
    },
    BrowserReady {
        aid: ActivityId,
        ok_or_err: Result<(), String>,
    },
    /// New frame written to shm. `lap` is the monotonic ring counter,
    /// `slot_idx = lap % NUM_SLOTS`.
    FrameDescriptor {
        aid: ActivityId,
        lap: u64,
        slot_idx: u8,
        frame_seq: u64,
        captured_at_us: u64,
        is_keyframe: bool,
        damage_rects: Vec<Rect>,
        is_popup: bool,
    },
}

/// frontend → daemon. PoC subset.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserClientMsg {
    /// Initial / reconnect handshake. Returns SubscribeReply.
    Subscribe {
        session_id: Option<u64>,
        last_key: Option<FrameKey>,
        has_base_keyframe: bool,
    },
    Resize {
        css_w: u32,
        css_h: u32,
        dpr: f32,
    },
}

/// daemon → frontend. PoC subset.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserServerMsg {
    /// Response to Subscribe.
    SubscribeReply {
        session_id: u64,
        result: FrameSubscriptionReply,
    },
    /// New screencast frame.
    Screencast {
        session_id: u64,
        epoch: u32,
        frame_seq: u64,
        captured_at_us: u64,
        width: u32,
        height: u32,
        is_keyframe: bool,
        damage_rects: Vec<Rect>,
        #[serde(with = "crate::bytes_serde")]
        bgra: Bytes,
    },
}

/// Result of subscribe_frames inside SubscribeReply.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameSubscriptionReply {
    /// Keyframe + new broadcast stream — subsequent Screencast messages follow.
    FreshSnapshot,
    /// No keyframe yet — waiting for first paint.
    AwaitingKeyframe,
}
