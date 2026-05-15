//! Pure translation between wire input events and chromiumoxide CDP types.
//!
//! Functions here are total and side-effect-free. They return `Option<...>` —
//! `None` if the input does not correspond to the translator's variant
//! (which is a contract violation by the caller, not a runtime failure).

use chromiumoxide::cdp::browser_protocol::input as cdp_input;

use crate::wire::{BrowserClientMsg, KeyKind, MouseButton, MouseKind};

/// Translates a `BrowserClientMsg::Mouse` to CDP `dispatchMouseEvent` params.
/// Returns `None` for any non-`Mouse` variant.
#[cfg_attr(not(test), expect(dead_code, reason = "wired up in Task 2.6"))]
pub(crate) fn mouse_to_cdp(msg: &BrowserClientMsg) -> Option<cdp_input::DispatchMouseEventParams> {
    let BrowserClientMsg::Mouse {
        mouse_kind,
        x,
        y,
        button,
        modifiers,
    } = msg
    else {
        return None;
    };

    let event_type = match mouse_kind {
        MouseKind::Down => cdp_input::DispatchMouseEventType::MousePressed,
        MouseKind::Up => cdp_input::DispatchMouseEventType::MouseReleased,
        MouseKind::Move => cdp_input::DispatchMouseEventType::MouseMoved,
    };

    let cdp_button = map_mouse_button(*button);

    // NOTE: click_count 1 for press/release, 0 for move — CDP requires this
    // field to be set for click events; movement events don't use it.
    let click_count: i64 = match mouse_kind {
        MouseKind::Down | MouseKind::Up => 1,
        MouseKind::Move => 0,
    };

    cdp_input::DispatchMouseEventParams::builder()
        .r#type(event_type)
        .x(*x)
        .y(*y)
        .button(cdp_button)
        .modifiers(i64::from(*modifiers))
        .click_count(click_count)
        .build()
        .ok()
}

/// Translates a `BrowserClientMsg::Wheel` to CDP `dispatchMouseEvent` params
/// (CDP exposes wheel events as a `MouseWheel` event type on the same method).
#[cfg_attr(not(test), expect(dead_code, reason = "wired up in Task 2.6"))]
pub(crate) fn wheel_to_cdp(msg: &BrowserClientMsg) -> Option<cdp_input::DispatchMouseEventParams> {
    let BrowserClientMsg::Wheel {
        x,
        y,
        dx,
        dy,
        modifiers,
    } = msg
    else {
        return None;
    };

    cdp_input::DispatchMouseEventParams::builder()
        .r#type(cdp_input::DispatchMouseEventType::MouseWheel)
        .x(*x)
        .y(*y)
        .button(cdp_input::MouseButton::None)
        .delta_x(*dx)
        .delta_y(*dy)
        .modifiers(i64::from(*modifiers))
        .click_count(0)
        .build()
        .ok()
}

/// Translates a `BrowserClientMsg::Key` to CDP `dispatchKeyEvent` params.
#[cfg_attr(not(test), expect(dead_code, reason = "wired up in Task 2.6"))]
pub(crate) fn key_to_cdp(msg: &BrowserClientMsg) -> Option<cdp_input::DispatchKeyEventParams> {
    let BrowserClientMsg::Key {
        key_kind,
        code,
        key,
        text,
        modifiers,
    } = msg
    else {
        return None;
    };

    let event_type = match key_kind {
        KeyKind::Down => cdp_input::DispatchKeyEventType::KeyDown,
        KeyKind::Up => cdp_input::DispatchKeyEventType::KeyUp,
    };

    let mut builder = cdp_input::DispatchKeyEventParams::builder()
        .r#type(event_type)
        .code(code.clone())
        .key(key.clone())
        .modifiers(i64::from(*modifiers));

    if let Some(t) = text {
        builder = builder.text(t.clone());
    }

    builder.build().ok()
}

/// Translates a `BrowserClientMsg::Paste` to CDP `insertText` params.
#[cfg_attr(not(test), expect(dead_code, reason = "wired up in Task 2.6"))]
pub(crate) fn paste_to_cdp(msg: &BrowserClientMsg) -> Option<cdp_input::InsertTextParams> {
    let BrowserClientMsg::Paste { text } = msg else {
        return None;
    };

    Some(cdp_input::InsertTextParams::new(text.clone()))
}

/// Translates a `BrowserClientMsg::ImeComposition` to CDP `imeSetComposition`
/// params. `replacement_start` / `replacement_end` are left as `None`.
#[cfg_attr(not(test), expect(dead_code, reason = "wired up in Task 2.6"))]
pub(crate) fn ime_composition_to_cdp(
    msg: &BrowserClientMsg,
) -> Option<cdp_input::ImeSetCompositionParams> {
    let BrowserClientMsg::ImeComposition {
        text,
        selection_start,
        selection_end,
    } = msg
    else {
        return None;
    };

    Some(cdp_input::ImeSetCompositionParams::new(
        text.clone(),
        i64::from(*selection_start),
        i64::from(*selection_end),
    ))
}

/// Translates a `BrowserClientMsg::ImeCommit` to CDP `insertText` params.
/// CDP has no separate "commit composition" method; `insertText` is the
/// correct path for committing composed text.
#[cfg_attr(not(test), expect(dead_code, reason = "wired up in Task 2.6"))]
pub(crate) fn ime_commit_to_cdp(msg: &BrowserClientMsg) -> Option<cdp_input::InsertTextParams> {
    let BrowserClientMsg::ImeCommit { text } = msg else {
        return None;
    };

    Some(cdp_input::InsertTextParams::new(text.clone()))
}

/// Maps our wire `MouseButton` to the chromiumoxide CDP `MouseButton`.
fn map_mouse_button(button: MouseButton) -> cdp_input::MouseButton {
    match button {
        MouseButton::Left => cdp_input::MouseButton::Left,
        MouseButton::Middle => cdp_input::MouseButton::Middle,
        MouseButton::Right => cdp_input::MouseButton::Right,
        MouseButton::None => cdp_input::MouseButton::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::*;

    fn mouse_down() -> BrowserClientMsg {
        BrowserClientMsg::Mouse {
            mouse_kind: MouseKind::Down,
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
            modifiers: 0,
        }
    }

    #[test]
    fn mouse_down_left_translates_to_cdp_press() {
        let p = mouse_to_cdp(&mouse_down()).expect("must translate");
        assert_eq!(p.x as i64, 100);
        assert_eq!(p.y as i64, 200);
    }

    #[test]
    fn mouse_to_cdp_returns_none_for_wheel_variant() {
        let wheel = BrowserClientMsg::Wheel {
            x: 0.0,
            y: 0.0,
            dx: 0.0,
            dy: 10.0,
            modifiers: 0,
        };
        assert!(mouse_to_cdp(&wheel).is_none());
    }

    #[test]
    fn wheel_to_cdp_carries_delta() {
        let p = wheel_to_cdp(&BrowserClientMsg::Wheel {
            x: 0.0,
            y: 0.0,
            dx: 0.0,
            dy: 120.0,
            modifiers: 0,
        })
        .expect("must translate");
        assert_eq!(p.x as i64, 0);
        assert_eq!(p.delta_y, Some(120.0));
    }

    #[test]
    fn key_down_with_text_carries_char() {
        let p = key_to_cdp(&BrowserClientMsg::Key {
            key_kind: KeyKind::Down,
            code: "KeyA".into(),
            key: "a".into(),
            text: Some("a".into()),
            modifiers: 0,
        })
        .expect("must translate");
        assert_eq!(p.text.as_deref(), Some("a"));
    }

    #[test]
    fn key_up_without_text_has_none_text() {
        let p = key_to_cdp(&BrowserClientMsg::Key {
            key_kind: KeyKind::Up,
            code: "KeyA".into(),
            key: "a".into(),
            text: None,
            modifiers: 0,
        })
        .expect("must translate");
        assert!(p.text.is_none());
    }

    #[test]
    fn paste_to_cdp_carries_text() {
        let p = paste_to_cdp(&BrowserClientMsg::Paste {
            text: "hello".into(),
        })
        .expect("must translate");
        assert_eq!(p.text, "hello");
    }

    #[test]
    fn ime_composition_passes_text_and_selection() {
        let msg = BrowserClientMsg::ImeComposition {
            text: "こん".into(),
            selection_start: 1,
            selection_end: 1,
        };
        let p = ime_composition_to_cdp(&msg).expect("must translate");
        assert_eq!(p.text, "こん");
        assert_eq!(p.selection_start, 1);
        assert_eq!(p.selection_end, 1);
    }

    #[test]
    fn ime_commit_carries_text() {
        let p = ime_commit_to_cdp(&BrowserClientMsg::ImeCommit {
            text: "確定".into(),
        })
        .expect("must translate");
        assert_eq!(p.text, "確定");
    }

    #[test]
    fn translators_return_none_for_unrelated_variants() {
        let nav = BrowserClientMsg::Nav {
            nav: NavCommand::Back,
        };
        assert!(mouse_to_cdp(&nav).is_none());
        assert!(wheel_to_cdp(&nav).is_none());
        assert!(key_to_cdp(&nav).is_none());
        assert!(paste_to_cdp(&nav).is_none());
        assert!(ime_composition_to_cdp(&nav).is_none());
        assert!(ime_commit_to_cdp(&nav).is_none());
    }
}
