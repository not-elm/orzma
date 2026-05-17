//! CefRenderHandler — writes OnPaint BGRA frames into the per-activity shm ring.

use crate::shm_writer::{MAX_DAMAGE_RECTS, ShmWriter, SlotData};
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
        shm: Arc<ShmWriter>,
        state: Arc<RenderHandlerState>,
        event_tx: mpsc::UnboundedSender<HostEvent>,
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

            let delta_payload: Vec<u8> = if is_keyframe {
                Vec::new()
            } else {
                let cap: usize = damage.iter().map(|r| (r.w * r.h * 4) as usize).sum();
                let mut out = Vec::with_capacity(cap);
                for r in &damage {
                    for row in 0..r.h {
                        let src_off = ((r.y + row) as usize) * stride + (r.x as usize) * 4;
                        let row_bytes = (r.w * 4) as usize;
                        out.extend_from_slice(&buf[src_off..src_off + row_bytes]);
                    }
                }
                out
            };
            let payload: &[u8] = if is_keyframe { buf } else { &delta_payload };

            let frame_seq = self.state.alloc_frame_seq();
            let captured_at_us = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0);

            let descriptor_damage = damage.clone();
            let (lap, slot_idx) = if is_popup {
                self.shm.write_slot_popup(SlotData {
                    frame_seq,
                    captured_at_us,
                    width: width as u32,
                    height: height as u32,
                    is_keyframe,
                    damage_rects: damage,
                    is_popup: true,
                    payload,
                });
                // NOTE: the popup slot is fixed; the daemon reads it via
                // read_popup, so lap / slot_idx carry no meaning here.
                (0u64, 0u8)
            } else {
                let lap = self.shm.current_lap();
                let slot_idx = self.shm.write_slot(SlotData {
                    frame_seq,
                    captured_at_us,
                    width: width as u32,
                    height: height as u32,
                    is_keyframe,
                    damage_rects: damage,
                    is_popup: false,
                    payload,
                });

                if is_keyframe {
                    self.state.force_keyframe.set(false);
                }
                (lap, slot_idx)
            };

            // Notify the daemon that a frame is ready in shm. A send error
            // means the control channel closed (daemon gone) — nothing to do.
            let _ = self.event_tx.send(HostEvent::FrameDescriptor {
                aid: self.aid.clone(),
                lap,
                slot_idx,
                frame_seq,
                captured_at_us,
                is_keyframe,
                damage_rects: descriptor_damage,
                is_popup,
            });

            tracing::debug!(
                aid = ?self.aid,
                frame_seq,
                is_popup,
                is_keyframe,
                width,
                height,
                payload_len = payload.len(),
                "on_paint -> shm",
            );
        }
    }
}
