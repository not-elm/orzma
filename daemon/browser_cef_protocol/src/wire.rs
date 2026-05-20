//! Wire schemas for daemon ↔ cef_host (HostCommand/HostEvent) and
//! daemon ↔ frontend (BrowserClientMsg/BrowserServerMsg).
//!
//! Phase A Task A15: all variants called out in Plan 2 spec §5 are present.

use crate::types::{ActivityId, FrameKey, Rect};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Semantic mouse-cursor kind the embedded page requests, mapped from CEF's
/// `cef_cursor_type_t` and rendered by the frontend as a Tailwind `cursor-*`
/// utility on the browser overlay. Custom cursor images are not represented —
/// `cef_host` falls back to `Default` for those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    /// The default arrow pointer.
    Default,
    /// Hand / link pointer (hovering a clickable element).
    Pointer,
    /// I-beam text-selection cursor.
    Text,
    /// Crosshair cursor.
    Crosshair,
    /// Busy / wait cursor.
    Wait,
    /// In-progress cursor (arrow with a spinner).
    Progress,
    /// Help / question-mark cursor.
    Help,
    /// Move cursor.
    Move,
    /// Action-not-allowed cursor.
    NotAllowed,
    /// Open-hand grab cursor.
    Grab,
    /// Closed-hand grabbing cursor.
    Grabbing,
    /// Horizontal column-resize cursor.
    ColResize,
    /// Vertical row-resize cursor.
    RowResize,
    /// Diagonal NE↔SW resize cursor.
    NeswResize,
    /// Diagonal NW↔SE resize cursor.
    NwseResize,
    /// Zoom-in cursor.
    ZoomIn,
    /// Zoom-out cursor.
    ZoomOut,
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

/// Wire representation of a Browser Activity storage profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserProfileWire {
    /// Disk-persistent named profile.
    Named {
        /// Profile name; resolved to a cache directory by cef_host.
        name: String,
    },
    /// Ephemeral in-memory profile.
    Incognito,
}

/// daemon → cef_host commands (spec §5).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostCommand {
    /// Initial daemon → cef_host configuration after Hello handshake.
    Ready {
        /// Absolute path to the per-PID runtime root directory.
        runtime_root: String,
    },
    /// Create a new BrowserActivity. The shm fd is passed out-of-band via
    /// SCM_RIGHTS; cookies are forwarded inline so cef_host can seed the
    /// cookie store (Task B12).
    BrowserCreate {
        /// Activity identifier.
        aid: ActivityId,
        /// URL to navigate to on creation.
        initial_url: String,
        /// Epoch counter for the associated shm ring.
        epoch: u32,
        /// Cookies to seed into the browser's cookie store.
        cookies: Vec<CefCookieDto>,
        /// Storage profile for this activity's browser.
        profile: BrowserProfileWire,
    },
    /// Replace the shm ring for an existing activity (e.g. after a cef_host
    /// respawn). The new shm fd is passed via SCM_RIGHTS.
    RecreateShm {
        /// Activity identifier.
        aid: ActivityId,
        /// New epoch value.
        new_epoch: u32,
    },
    /// Navigate an activity to a new URL.
    Navigate {
        /// Activity identifier.
        aid: ActivityId,
        /// Target URL.
        url: String,
    },
    /// Navigate back (`delta < 0`) or forward (`delta > 0`) in history.
    NavigateHistory {
        /// Activity identifier.
        aid: ActivityId,
        /// Steps to move; negative is backward, positive is forward.
        delta: i64,
    },
    /// Notify cef_host of a viewport resize.
    Resize {
        /// Activity identifier.
        aid: ActivityId,
        /// CSS pixel width.
        css_w: u32,
        /// CSS pixel height.
        css_h: u32,
        /// Device pixel ratio.
        dpr: f32,
    },
    /// Forward a user input event to cef_host.
    SendInput {
        /// Activity identifier.
        aid: ActivityId,
        /// The input event payload.
        input: InputEvent,
    },
    /// Ask cef_host to stop producing screencast frames for this activity.
    PauseScreencast {
        /// Activity identifier.
        aid: ActivityId,
    },
    /// Ask cef_host to resume producing screencast frames for this activity.
    ResumeScreencast {
        /// Activity identifier.
        aid: ActivityId,
    },
    /// Request the current text selection for this activity.
    GetSelection {
        /// Activity identifier.
        aid: ActivityId,
        /// Opaque token echoed back in `HostEvent::SelectionChanged`.
        request_id: u64,
    },
    /// Write text to the system clipboard on behalf of the activity.
    SetClipboard {
        /// Text to place on the clipboard.
        text: String,
    },
    /// Destroy a BrowserActivity and free its resources.
    Close {
        /// Activity identifier.
        aid: ActivityId,
    },
    /// Graceful shutdown request; cef_host should tear down and exit.
    Shutdown,
}

/// cef_host → daemon events (spec §5).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostEvent {
    /// Initial handshake greeting from cef_host.
    Hello {
        /// Chromium / CEF version string.
        cef_version: String,
        /// Protocol ABI version for compatibility checking.
        abi_version: u32,
        /// PID of the cef_host process.
        pid: u32,
    },
    /// Outcome of a `BrowserCreate` command.
    BrowserReady {
        /// Activity identifier.
        aid: ActivityId,
        /// `Ok(())` on success; `Err(msg)` if creation failed.
        ok_or_err: Result<(), String>,
    },
    /// New frame written to shm. `lap` is the monotonic ring counter,
    /// `slot_idx = lap % NUM_SLOTS`.
    FrameDescriptor {
        /// Activity identifier.
        aid: ActivityId,
        /// Monotonic lap counter across all shm slots.
        lap: u64,
        /// Index of the shm slot holding this frame (`lap % NUM_SLOTS`).
        slot_idx: u8,
        /// Monotonic frame sequence within the current epoch.
        frame_seq: u64,
        /// Wall-clock capture timestamp in microseconds.
        captured_at_us: u64,
        /// `true` for keyframes that can begin a decode chain.
        is_keyframe: bool,
        /// Damaged pixel regions within the frame.
        damage_rects: Vec<Rect>,
        /// `true` when this frame belongs to a popup overlay.
        is_popup: bool,
    },
    /// In-process screencast frame carrying its pixel payload inline as
    /// `bytes::Bytes` (Plan 3 Task 11+12). Replaces `FrameDescriptor` on the
    /// in-process path: the cef_host render handler copies the BGRA buffer
    /// into a recycled `Vec<u8>` from `FrameBufferPool`, wraps it in `Bytes`,
    /// and sends this event so the event pump can push a `FrameEnvelope`
    /// straight into the per-activity `FrameRing` without any shm hop.
    ///
    /// Fields mirror `FrameEnvelope` 1:1 to avoid a circular `ozmux_browser`
    /// dependency on this crate. Plan 5 Task 22 will retire `FrameDescriptor`.
    FrameProduced {
        /// Activity identifier.
        aid: ActivityId,
        /// Daemon-wide session identifier stamped on this frame.
        session_id: u64,
        /// Monotonic epoch counter; increments on cef_host respawn.
        epoch: u32,
        /// Monotonic frame sequence counter within the current epoch.
        frame_seq: u64,
        /// Wall-clock capture timestamp in microseconds.
        captured_at_us: u64,
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
        /// `true` for keyframes that can begin a decode chain.
        is_keyframe: bool,
        /// Damaged pixel regions within the frame.
        damage_rects: Vec<Rect>,
        /// `true` when this frame belongs to a popup overlay.
        is_popup: bool,
        /// Raw BGRA pixel data (full frame for keyframes; concatenated
        /// damaged-row strips for deltas).
        #[serde(with = "crate::bytes_serde")]
        bgra: Bytes,
    },
    /// Navigation state changed (URL, title, back/forward availability).
    NavStateChanged {
        /// Activity identifier.
        aid: ActivityId,
        /// Current page URL.
        url: String,
        /// Current page title.
        title: String,
        /// `true` if back navigation is available.
        can_back: bool,
        /// `true` if forward navigation is available.
        can_forward: bool,
    },
    /// Page title changed without a full navigation.
    TitleChanged {
        /// Activity identifier.
        aid: ActivityId,
        /// New page title.
        title: String,
    },
    /// The mouse cursor the embedded page requests changed (e.g. a link
    /// hover switching to the hand pointer).
    CursorChanged {
        /// Activity identifier.
        aid: ActivityId,
        /// New cursor kind.
        cursor: CursorKind,
    },
    /// Reply to `HostCommand::GetSelection`.
    SelectionChanged {
        /// Activity identifier.
        aid: ActivityId,
        /// Currently selected text (empty if nothing is selected).
        text: String,
    },
    /// A page-level error occurred (e.g. HTTP error, navigation failure).
    PageError {
        /// Activity identifier.
        aid: ActivityId,
        /// Net error code (negative Chromium net:: code).
        code: i32,
        /// Human-readable error description.
        error_text: String,
    },
    /// The renderer process for this activity exited unexpectedly.
    RenderProcessTerminated {
        /// Activity identifier.
        aid: ActivityId,
        /// Human-readable reason string.
        reason: String,
    },
    /// A CEF / Chromium log line emitted by the child process.
    LogLine {
        /// Log severity level (`"INFO"`, `"WARNING"`, `"ERROR"`, etc.).
        level: String,
        /// Log message text.
        text: String,
    },
    /// cef_host process-level crash (e.g. browser process died).
    Crashed {
        /// Human-readable description of the crash.
        reason: String,
    },
}

/// A user input event forwarded from the frontend to cef_host via
/// `HostCommand::SendInput`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// Pointer moved without a button press.
    MouseMove {
        /// Pointer X in CSS pixels.
        x: i32,
        /// Pointer Y in CSS pixels.
        y: i32,
        /// Active modifier bitmask (CEF `cef_event_flags_t`).
        modifiers: u32,
    },
    /// Button press or release.
    MouseClick {
        /// Pointer X in CSS pixels.
        x: i32,
        /// Pointer Y in CSS pixels.
        y: i32,
        /// Which button was used.
        button: MouseButton,
        /// Click count (1 for single, 2 for double, etc.).
        count: u32,
        /// `true` for mouse-up, `false` for mouse-down.
        mouse_up: bool,
        /// Active modifier bitmask.
        modifiers: u32,
    },
    /// Scroll wheel event.
    MouseWheel {
        /// Pointer X in CSS pixels.
        x: i32,
        /// Pointer Y in CSS pixels.
        y: i32,
        /// Horizontal scroll delta.
        delta_x: i32,
        /// Vertical scroll delta.
        delta_y: i32,
        /// Active modifier bitmask.
        modifiers: u32,
    },
    /// Keyboard event.
    Key {
        // NOTE: the outer InputEvent tag uses `"kind"` (via `#[serde(tag = "kind")]`).
        // The spec named this inner field `kind` too, which would collide. We use
        // `event_type` in Rust and the frontend's `protocol/input.ts` remaps it.
        /// Key event phase (raw-down, key-up, or char).
        event_type: KeyEventType,
        /// Windows virtual-key code.
        windows_key_code: i32,
        /// Platform-native key code.
        native_key_code: i32,
        /// Active modifier bitmask (CEF `cef_event_flags_t`).
        modifiers: u32,
        /// Character produced (UTF-16 code unit).
        character: u16,
        /// Character produced without modifiers (UTF-16 code unit).
        unmodified_character: u16,
        /// `true` when the focus is inside an editable field.
        focus_on_editable_field: bool,
    },
    /// IME composition update (preedit text changed).
    ImeSetComposition {
        /// Current preedit text.
        text: String,
        /// Underline style ranges within the preedit text.
        underlines: Vec<ImeUnderline>,
        /// Replacement range `(from, to)`; `(-1, -1)` means no replacement.
        /// Task B2 translates this to `Option<cef_rs::Range>` when calling into CEF.
        replacement_range: (i32, i32),
        /// Cursor + selection within the preedit string.
        selection_range: (i32, i32),
    },
    /// IME composition committed (preedit text accepted).
    ImeCommit {
        /// Committed text.
        text: String,
        /// Optional replacement range `(from, to)` in the existing document text.
        replacement_range: Option<(i32, i32)>,
        /// Cursor position relative to the insertion point after commit.
        relative_cursor_pos: i32,
    },
    /// IME composition cancelled (preedit text discarded).
    ImeCancel,
}

/// Phase of a keyboard event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyEventType {
    /// Raw key-down before character generation (CEF `KEYEVENT_RAWKEYDOWN`).
    RawKeyDown,
    /// Key released (CEF `KEYEVENT_KEYUP`).
    KeyUp,
    /// Character produced (CEF `KEYEVENT_CHAR`).
    Char,
}

/// Mouse button identifier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Primary (left) button.
    Left,
    /// Middle (wheel) button.
    Middle,
    /// Secondary (right) button.
    Right,
}

/// Underline styling for an IME composition range.
///
/// Field names match the `cef_rs` 148 `Range` type (`from: u32`, `to: u32`)
/// as confirmed by the A12 spike.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImeUnderline {
    /// Start of the underlined range (inclusive, in UTF-16 code units).
    pub from: u32,
    /// End of the underlined range (exclusive, in UTF-16 code units).
    pub to: u32,
    /// Underline color as `0xAARRGGBB`.
    pub color: u32,
    /// Background color as `0xAARRGGBB`.
    pub background_color: u32,
    /// `true` for thick underlines (active segment in some IMEs).
    pub thick: bool,
}

/// frontend → daemon messages (spec §5).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserClientMsg {
    /// Initial / reconnect handshake. Returns `BrowserServerMsg::SubscribeReply`.
    Subscribe {
        /// Session id from a previous connection, or `None` for a fresh start.
        session_id: Option<u64>,
        /// Last frame key held by the frontend for delta replay, or `None`.
        last_key: Option<FrameKey>,
        /// `true` when the frontend holds a decoded keyframe in its renderer.
        has_base_keyframe: bool,
    },
    /// Viewport resize notification from the frontend.
    Resize {
        /// CSS pixel width.
        css_w: u32,
        /// CSS pixel height.
        css_h: u32,
        /// Device pixel ratio.
        dpr: f32,
    },
    /// Forwarded user input event.
    Input {
        /// The input event payload.
        event: InputEvent,
    },
    /// Navigate to a URL (typed into the toolbar or injected by an extension).
    Navigate {
        /// Target URL.
        url: String,
    },
    /// Navigate back or forward in history.
    NavigateHistory {
        /// Steps; negative is backward, positive is forward.
        delta: i64,
    },
    /// Request the current text selection (triggers `SelectionChanged` reply).
    CopyRequest,
    /// Paste text into the focused element.
    Paste {
        /// Text to paste.
        text: String,
    },
}

/// daemon → frontend messages (spec §5).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserServerMsg {
    /// Response to `BrowserClientMsg::Subscribe`.
    SubscribeReply {
        /// Session id for this connection.
        session_id: u64,
        /// Outcome of the subscription attempt.
        result: FrameSubscriptionReply,
    },
    /// A decoded screencast frame ready for display.
    Screencast {
        /// Session id this frame belongs to.
        session_id: u64,
        /// Epoch counter; changes on cef_host respawn.
        epoch: u32,
        /// Monotonic frame sequence within the epoch.
        frame_seq: u64,
        /// Wall-clock capture timestamp in microseconds.
        captured_at_us: u64,
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
        /// `true` for keyframes that can begin a decode chain.
        is_keyframe: bool,
        /// Damaged pixel regions within the frame.
        damage_rects: Vec<Rect>,
        /// `true` when this frame is for a popup overlay (e.g. select dropdown).
        is_popup: bool,
        /// Bounding rect of the popup relative to the main viewport, if known.
        popup_rect: Option<Rect>,
        /// Raw BGRA pixel data.
        #[serde(with = "crate::bytes_serde")]
        bgra: Bytes,
    },
    /// Viewport dimensions changed (e.g. after a resize round-trip).
    Viewport {
        /// New viewport width in pixels.
        width: u32,
        /// New viewport height in pixels.
        height: u32,
    },
    /// Navigation state update (URL, title, history availability).
    Nav {
        /// Current page URL.
        url: String,
        /// Current page title.
        title: String,
        /// `true` if back navigation is available.
        can_back: bool,
        /// `true` if forward navigation is available.
        can_forward: bool,
    },
    /// The mouse cursor the embedded page requests changed. The frontend
    /// applies the matching `cursor-*` utility to the browser overlay.
    Cursor {
        /// New cursor kind.
        cursor: CursorKind,
    },
    /// Current text selection, sent in reply to `BrowserClientMsg::CopyRequest`.
    SelectionChanged {
        /// Selected text (empty if nothing selected).
        text: String,
    },
    /// Write text to the system clipboard (issued by a page `document.execCommand`).
    ClipboardWrite {
        /// Text to place on the clipboard.
        text: String,
    },
    /// A page-level error occurred.
    PageError {
        /// Net error code.
        code: i32,
        /// Human-readable error description.
        error_text: String,
    },
    /// The renderer process exited unexpectedly.
    RendererTerminated {
        /// Human-readable reason.
        reason: String,
    },
    /// The browser backend is not available and cannot recover.
    BrowserUnavailable {
        /// Why the browser is unavailable.
        reason: BrowserUnavailableReason,
    },
}

/// Reason sent in `BrowserServerMsg::BrowserUnavailable` (spec §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserUnavailableReason {
    /// All spawn/restart attempts for cef_host have been exhausted.
    RetryExhausted {
        /// The last error message that caused the final failure.
        last_error: String,
    },
    /// The cef_host binary was not found at the expected path.
    BinaryNotFound {
        /// Absolute path that was searched.
        path: PathBuf,
    },
    /// `CefInitialize` returned a failure exit code.
    CefInitFailed {
        /// The exit code returned by cef_host.
        exit_code: i32,
    },
    /// cef_host reported an ABI version incompatible with the daemon.
    ProtocolMismatch {
        /// ABI version the daemon requires.
        expected: u32,
        /// ABI version cef_host advertised.
        got: u32,
    },
}

/// Result of `subscribe_frames` inside `BrowserServerMsg::SubscribeReply`
/// (wire mirror of `daemon/browser::frame_ring::FrameSubscription`; spec §5
/// + parent §15).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameSubscriptionReply {
    /// Keyframe + (optional) replay deltas + live stream follow.
    FreshSnapshot,
    /// Replay deltas + live stream follow; frontend already holds a keyframe.
    ResumeReplay,
    /// Subscription rejected. Frontend must drop state and re-subscribe with
    /// `last_key=None`.
    MustRestart {
        /// Why the daemon refused the resume request.
        reason: MustRestartReason,
    },
    /// Ring not warm yet — frontend waits for the first keyframe to arrive
    /// on the live broadcast.
    AwaitingKeyframe,
}

/// Reason `FrameSubscriptionReply::MustRestart` was returned.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MustRestartReason {
    /// Client's `session_id` does not match the daemon's session.
    SessionMismatch,
    /// `last_key.epoch` does not match the ring's current epoch.
    EpochMismatch,
    /// `last_key` was once present in the ring but has been evicted.
    LastKeyEvicted,
}
