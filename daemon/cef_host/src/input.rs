//! Wire-to-CEF input event mapping.
//!
//! Translates [`InputEvent`] variants received over the daemon ↔ cef_host
//! control plane into the corresponding cef-rs 148 `BrowserHost` method calls.
//! Called from `BrowserPool::execute` on the CEF UI thread.
//!
//! IME underline colours are forwarded as an empty slice for now; Plan 3
//! wires coloured underlines using the `ImeUnderline` wire type (A12 spike
//! confirmed `CompositionUnderline::default()` sets the required `size`).

use cef::{
    CefString, CompositionUnderline, ImplBrowser, ImplBrowserHost, KeyEvent, KeyEventType,
    MouseButtonType, MouseEvent, PaintElementType, Range,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{InputEvent, MouseButton};
use std::os::raw::c_int;

/// Converts a wire replacement-range pair into a cef-rs [`Range`].
///
/// Returns `None` when the wire sentinel `(-1, -1)` indicates "no replacement
/// range". Otherwise clamps negative values to zero and casts to `u32`.
fn wire_range_to_cef(r: (i32, i32)) -> Option<Range> {
    if r == (-1, -1) {
        None
    } else {
        Some(Range {
            from: r.0.max(0) as u32,
            to: r.1.max(0) as u32,
        })
    }
}

/// Dispatches a wire [`InputEvent`] to the appropriate cef-rs `BrowserHost` method.
///
/// Must be called from the CEF UI thread. Logs a warning if the host is
/// unavailable and returns without panicking.
pub fn dispatch(browser: &cef::Browser, aid: &ActivityId, event: InputEvent) {
    let host = match browser.host() {
        Some(h) => h,
        None => {
            tracing::warn!(?aid, "dispatch: browser host unavailable");
            return;
        }
    };

    match event {
        InputEvent::MouseMove { x, y, modifiers } => {
            let ev = MouseEvent { x, y, modifiers };
            host.send_mouse_move_event(Some(&ev), 0 as c_int);
        }
        InputEvent::MouseClick {
            x,
            y,
            button,
            count,
            mouse_up,
            modifiers,
        } => {
            let ev = MouseEvent { x, y, modifiers };
            let btn = wire_button_to_cef(button);
            let up: c_int = if mouse_up { 1 } else { 0 };
            // NOTE: click_count is u32 on the wire; saturating cast to c_int is safe
            // because realistic click counts are well within i32 range.
            let click_count = count.min(i32::MAX as u32) as c_int;
            host.send_mouse_click_event(Some(&ev), btn, up, click_count);
        }
        InputEvent::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => {
            let ev = MouseEvent { x, y, modifiers };
            host.send_mouse_wheel_event(Some(&ev), delta_x as c_int, delta_y as c_int);
        }
        InputEvent::Key {
            event_type,
            windows_key_code,
            native_key_code,
            modifiers,
            character,
            unmodified_character,
            focus_on_editable_field,
        } => {
            let cef_type = wire_key_type_to_cef(event_type);
            let key_ev = KeyEvent {
                type_: cef_type,
                modifiers,
                windows_key_code,
                native_key_code,
                // NOTE: character and unmodified_character are u16 on the wire
                // and char16_t (u16) in cef-rs 148 — direct assignment is correct.
                character,
                unmodified_character,
                focus_on_editable_field: if focus_on_editable_field { 1 } else { 0 },
                ..KeyEvent::default()
            };
            host.send_key_event(Some(&key_ev));
        }
        InputEvent::ImeSetComposition {
            text,
            underlines: _,
            replacement_range,
            selection_range,
        } => {
            let cef_text = CefString::from(text.as_str());
            let replacement = wire_range_to_cef(replacement_range);
            let selection = wire_range_to_cef(selection_range);
            // NOTE: underlines are passed as empty for now; Plan 3 maps
            // ImeUnderline wire structs to CompositionUnderline with correct
            // size (confirmed by A12 spike: CompositionUnderline::default()
            // sets the required sizeof(_cef_composition_underline_t)).
            let underlines: &[CompositionUnderline] = &[];
            host.ime_set_composition(
                Some(&cef_text),
                Some(underlines),
                replacement.as_ref(),
                selection.as_ref(),
            );
        }
        InputEvent::ImeCommit {
            text,
            replacement_range,
            relative_cursor_pos,
        } => {
            let cef_text = CefString::from(text.as_str());
            let replacement = replacement_range.and_then(wire_range_to_cef);
            // NOTE: A12 spike confirmed ime_commit_text is the correct call
            // for ImeCommit (not ime_finish_composing_text).
            host.ime_commit_text(
                Some(&cef_text),
                replacement.as_ref(),
                relative_cursor_pos as c_int,
            );
        }
        InputEvent::ImeCancel => {
            host.ime_cancel_composition();
        }
    }
}

fn wire_button_to_cef(button: MouseButton) -> MouseButtonType {
    match button {
        MouseButton::Left => MouseButtonType::LEFT,
        MouseButton::Middle => MouseButtonType::MIDDLE,
        MouseButton::Right => MouseButtonType::RIGHT,
    }
}

fn wire_key_type_to_cef(
    event_type: ozmux_browser_cef_protocol::wire::KeyEventType,
) -> KeyEventType {
    match event_type {
        ozmux_browser_cef_protocol::wire::KeyEventType::RawKeyDown => KeyEventType::RAWKEYDOWN,
        ozmux_browser_cef_protocol::wire::KeyEventType::KeyUp => KeyEventType::KEYUP,
        ozmux_browser_cef_protocol::wire::KeyEventType::Char => KeyEventType::CHAR,
    }
}

/// Forces a full repaint of the view surface for the given browser.
///
/// Called after `ResumeScreencast` to generate a fresh keyframe.
pub fn invalidate_view(browser: &cef::Browser, aid: &ActivityId) {
    let host = match browser.host() {
        Some(h) => h,
        None => {
            tracing::warn!(?aid, "invalidate_view: browser host unavailable");
            return;
        }
    };
    host.invalidate(PaintElementType::VIEW);
}
