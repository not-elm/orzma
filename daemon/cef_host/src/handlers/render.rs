//! CefRenderHandler — copies OnPaint BGRA frames out of CEF via the
//! `FrameBufferPool` and emits `HostEvent::FrameProduced` to the daemon.
//!
//! Plan 3 Task 11+12: replaces the Plan 1-2 shm-ring path. The handler still
//! holds an `Arc<ShmWriter>` so the legacy `FrameDescriptor` writer is kept
//! around for Plan 5 Task 22 to remove cleanly; on the hot path each frame
//! flows through the in-process pool + `Bytes` instead.

use crate::frame_buffer_pool::FrameBufferPool;
use crate::shm_writer::MAX_DAMAGE_RECTS;
use cef::rc::Rc as _;
use cef::{
    Browser, ImplRenderHandler, PaintElementType, Rect, RenderHandler, ScreenInfo,
    WrapRenderHandler, wrap_render_handler,
};
use cef_dll_sys::cef_paint_element_type_t;
use ozmux_browser_cef_protocol::types::{ActivityId, Rect as ProtoRect};
use ozmux_browser_cef_protocol::wire::HostEvent;
use std::cell::Cell;
use std::os::raw::c_int;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Per-browser render state.
///
/// All fields use `Cell` because this struct is only ever accessed from the
/// CEF UI thread. `Arc<RenderHandlerState>` is used so the pool can share the
/// state reference with the handler.
///
/// # Safety
/// `Sync` is safe because all access is from the CEF UI thread only.
pub struct RenderHandlerState {
    /// Viewport width in CSS pixels.
    pub width: Cell<u32>,
    /// Viewport height in CSS pixels.
    pub height: Cell<u32>,
    /// Device pixel ratio.
    pub dpr: Cell<f32>,
    /// Monotonically increasing frame sequence counter.
    pub next_frame_seq: Cell<u64>,
    /// When true, the next paint is forced to be a keyframe regardless of damage.
    pub force_keyframe: Cell<bool>,
    /// `true` while the popup widget (e.g. `<select>` dropdown) is visible.
    /// Set by `on_popup_show`; cleared on `on_popup_show(false)`.
    pub is_popup_visible: Cell<bool>,
    /// Most recent popup rect in main-view coordinates, from `on_popup_size`.
    /// Travels on every Screencast frame so the frontend can position the
    /// overlay canvas without a separate event.
    pub popup_rect: Cell<Option<ProtoRect>>,
}

impl RenderHandlerState {
    /// Creates a new state with the given viewport size.
    ///
    /// `force_keyframe` is initialized to `true` so the first paint is always a full frame.
    pub fn new(width: u32, height: u32, dpr: f32) -> Self {
        Self {
            width: Cell::new(width),
            height: Cell::new(height),
            dpr: Cell::new(dpr),
            next_frame_seq: Cell::new(1),
            force_keyframe: Cell::new(true),
            is_popup_visible: Cell::new(false),
            popup_rect: Cell::new(None),
        }
    }

    /// Allocates and returns the next frame sequence number.
    pub fn alloc_frame_seq(&self) -> u64 {
        let s = self.next_frame_seq.get();
        self.next_frame_seq.set(s + 1);
        s
    }
}

// SAFETY: `RenderHandlerState` is only accessed from the CEF UI thread. The
// `Cell` fields are not exposed to any other thread at runtime.
unsafe impl Sync for RenderHandlerState {}

wrap_render_handler! {
    pub struct OzmuxRenderHandler {
        aid: ActivityId,
        state: Arc<RenderHandlerState>,
        event_tx: mpsc::UnboundedSender<HostEvent>,
        frame_pool: Arc<FrameBufferPool>,
        session_id: u64,
        epoch: u32,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(r) = rect {
                r.x = 0;
                r.y = 0;
                r.width = self.state.width.get() as c_int;
                r.height = self.state.height.get() as c_int;
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> c_int {
            let Some(i) = screen_info else { return 0 };
            i.device_scale_factor = self.state.dpr.get();
            i.depth = 24;
            i.depth_per_component = 8;
            i.is_monochrome = 0;
            i.rect = Rect::default();
            i.available_rect = Rect::default();
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: c_int) {
            let visible = show != 0;
            self.state.is_popup_visible.set(visible);
            if !visible {
                self.state.popup_rect.set(None);
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(r) = rect {
                self.state.popup_rect.set(Some(ProtoRect {
                    x: r.x as u32,
                    y: r.y as u32,
                    w: r.width as u32,
                    h: r.height as u32,
                }));
            }
        }

        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: c_int,
            height: c_int,
        ) {
            let is_popup = matches!(type_.as_ref(), cef_paint_element_type_t::PET_POPUP);

            let stride = (width * 4) as usize;
            let total_len = (height as usize) * stride;
            // SAFETY: CEF guarantees the buffer is valid for the entire duration of
            // the on_paint callback. We must copy all bytes before returning.
            let buf = unsafe { std::slice::from_raw_parts(buffer, total_len) };

            let damage: Vec<ProtoRect> = dirty_rects
                .map(|rs| {
                    rs.iter()
                        .map(|r| ProtoRect {
                            x: r.x as u32,
                            y: r.y as u32,
                            w: r.width as u32,
                            h: r.height as u32,
                        })
                        .collect()
                })
                .unwrap_or_default();

            let full_screen = damage.iter().any(|r| {
                r.x == 0 && r.y == 0 && r.w == width as u32 && r.h == height as u32
            });
            let overflow = damage.len() > MAX_DAMAGE_RECTS;
            let is_keyframe = damage.is_empty()
                || full_screen
                || overflow
                || self.state.force_keyframe.get();

            // Acquire a recycled buffer sized to the frame's payload (keyframe =
            // whole framebuffer; delta = concatenated dirty rows). Copying out
            // before returning is mandatory: CEF reclaims `buffer` on return.
            let payload_len: usize = if is_keyframe {
                total_len
            } else {
                damage.iter().map(|r| (r.w * r.h * 4) as usize).sum()
            };
            let mut payload_buf = self.frame_pool.acquire(payload_len);
            if is_keyframe {
                payload_buf.copy_from_slice(buf);
            } else {
                let mut cursor = 0usize;
                for r in &damage {
                    let row_bytes = (r.w * 4) as usize;
                    for row in 0..r.h {
                        let src_off = ((r.y + row) as usize) * stride + (r.x as usize) * 4;
                        payload_buf[cursor..cursor + row_bytes]
                            .copy_from_slice(&buf[src_off..src_off + row_bytes]);
                        cursor += row_bytes;
                    }
                }
            }
            let bgra = bytes::Bytes::from(payload_buf);

            let frame_seq = self.state.alloc_frame_seq();
            let captured_at_us = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0);

            if is_keyframe && !is_popup {
                self.state.force_keyframe.set(false);
            }

            let payload_len_for_log = bgra.len();
            // A send error means the control channel closed (daemon gone) —
            // nothing to do; the recycled buffer's allocation ends up dropped
            // along with the unsent `Bytes`.
            let _ = self.event_tx.send(HostEvent::FrameProduced {
                aid: self.aid.clone(),
                session_id: self.session_id,
                epoch: self.epoch,
                frame_seq,
                captured_at_us,
                width: width as u32,
                height: height as u32,
                is_keyframe,
                damage_rects: damage,
                is_popup,
                bgra,
            });

            tracing::debug!(
                aid = ?self.aid,
                frame_seq,
                is_popup,
                is_keyframe,
                width,
                height,
                payload_len = payload_len_for_log,
                "on_paint -> FrameProduced",
            );
        }
    }
}
