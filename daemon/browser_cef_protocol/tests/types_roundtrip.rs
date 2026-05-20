use ozmux_browser_cef_protocol::types::{ActivityId, FrameKey, Rect};
use ozmux_browser_cef_protocol::wire::{
    BrowserClientMsg, BrowserExtraContext, BrowserRole, BrowserServerMsg,
    BrowserUnavailableReason, CursorKind, FrameSubscriptionReply, InputEvent, MouseButton,
    MustRestartReason,
};

#[test]
fn activity_id_roundtrips() {
    let v = ActivityId("a1".into());
    let bytes = rmp_serde::to_vec_named(&v).unwrap();
    let back: ActivityId = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(v, back);
}

#[test]
fn rect_roundtrips() {
    let v = Rect {
        x: 10,
        y: 20,
        w: 100,
        h: 200,
    };
    let bytes = rmp_serde::to_vec_named(&v).unwrap();
    let back: Rect = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(v, back);
}

#[test]
fn frame_key_roundtrips() {
    let v = FrameKey {
        session_id: 42,
        epoch: 1,
        frame_seq: 100,
    };
    let bytes = rmp_serde::to_vec_named(&v).unwrap();
    let back: FrameKey = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(v, back);
}

// --- WS schema tests (daemon ↔ frontend) ---

fn wire_roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(v: T) -> Vec<u8> {
    let bytes = rmp_serde::to_vec_named(&v).expect("serialize");
    let back: T = rmp_serde::from_slice(&bytes).expect("deserialize");
    let bytes2 = rmp_serde::to_vec_named(&back).expect("re-serialize");
    assert_eq!(bytes, bytes2, "roundtrip serialization not deterministic");
    bytes
}

// --- BrowserClientMsg variants ---

#[test]
fn browser_client_subscribe_roundtrips() {
    wire_roundtrip(BrowserClientMsg::Subscribe {
        session_id: Some(42),
        last_key: Some(FrameKey {
            session_id: 42,
            epoch: 1,
            frame_seq: 99,
        }),
        has_base_keyframe: true,
    });
}

#[test]
fn browser_client_msg_input_roundtrips() {
    wire_roundtrip(BrowserClientMsg::Input {
        event: InputEvent::MouseMove {
            x: 10,
            y: 20,
            modifiers: 0,
        },
    });
}

#[test]
fn browser_client_msg_input_mouse_click_roundtrips() {
    for btn in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
        wire_roundtrip(BrowserClientMsg::Input {
            event: InputEvent::MouseClick {
                x: 0,
                y: 0,
                button: btn,
                count: 1,
                mouse_up: true,
                modifiers: 0,
            },
        });
    }
}

#[test]
fn browser_client_msg_navigate_roundtrips() {
    wire_roundtrip(BrowserClientMsg::Navigate {
        url: "https://example.com/".into(),
    });
}

#[test]
fn browser_client_msg_navigate_history_roundtrips() {
    wire_roundtrip(BrowserClientMsg::NavigateHistory { delta: -2 });
    wire_roundtrip(BrowserClientMsg::NavigateHistory { delta: 3 });
}

#[test]
fn browser_client_msg_copy_request_roundtrips() {
    wire_roundtrip(BrowserClientMsg::CopyRequest);
}

#[test]
fn browser_client_msg_paste_roundtrips() {
    wire_roundtrip(BrowserClientMsg::Paste {
        text: "pasted content".into(),
    });
}

// --- BrowserServerMsg variants ---

#[test]
fn browser_server_screencast_with_bgra_payload_roundtrips() {
    use bytes::Bytes;
    let bgra = Bytes::from([0u8, 64, 128, 255].repeat(1280 * 800));
    wire_roundtrip(BrowserServerMsg::Screencast {
        session_id: 42,
        epoch: 1,
        frame_seq: 100,
        captured_at_us: 1,
        width: 1280,
        height: 800,
        is_keyframe: true,
        damage_rects: vec![],
        is_popup: false,
        popup_rect: None,
        bgra,
    });
}

#[test]
fn browser_server_msg_screencast_with_popup_roundtrips() {
    use bytes::Bytes;
    let bgra = Bytes::from(vec![0u8, 0, 0, 255]);
    wire_roundtrip(BrowserServerMsg::Screencast {
        session_id: 7,
        epoch: 2,
        frame_seq: 5,
        captured_at_us: 999,
        width: 200,
        height: 150,
        is_keyframe: true,
        damage_rects: vec![],
        is_popup: true,
        popup_rect: Some(Rect {
            x: 10,
            y: 20,
            w: 200,
            h: 150,
        }),
        bgra,
    });
}

#[test]
fn browser_server_subscribe_reply_fresh_snapshot_roundtrips() {
    wire_roundtrip(BrowserServerMsg::SubscribeReply {
        session_id: 42,
        result: FrameSubscriptionReply::FreshSnapshot,
    });
}

#[test]
fn frame_subscription_reply_all_variants_roundtrip() {
    for variant in [
        FrameSubscriptionReply::FreshSnapshot,
        FrameSubscriptionReply::ResumeReplay,
        FrameSubscriptionReply::MustRestart {
            reason: MustRestartReason::SessionMismatch,
        },
        FrameSubscriptionReply::MustRestart {
            reason: MustRestartReason::EpochMismatch,
        },
        FrameSubscriptionReply::MustRestart {
            reason: MustRestartReason::LastKeyEvicted,
        },
        FrameSubscriptionReply::AwaitingKeyframe,
    ] {
        let bytes = rmp_serde::to_vec_named(&variant).expect("serialize");
        let back: FrameSubscriptionReply = rmp_serde::from_slice(&bytes).expect("deserialize");
        let bytes2 = rmp_serde::to_vec_named(&back).expect("re-serialize");
        assert_eq!(
            bytes, bytes2,
            "FrameSubscriptionReply variant not roundtrip-stable"
        );
    }
}

#[test]
fn browser_server_msg_viewport_roundtrips() {
    wire_roundtrip(BrowserServerMsg::Viewport {
        width: 1280,
        height: 800,
    });
}

#[test]
fn browser_server_msg_nav_roundtrips() {
    wire_roundtrip(BrowserServerMsg::Nav {
        url: "https://example.com/".into(),
        title: "Example".into(),
        can_back: true,
        can_forward: false,
    });
}

#[test]
fn browser_server_msg_cursor_roundtrips() {
    wire_roundtrip(BrowserServerMsg::Cursor {
        cursor: CursorKind::Text,
    });
}

#[test]
fn browser_server_msg_selection_changed_roundtrips() {
    wire_roundtrip(BrowserServerMsg::SelectionChanged {
        text: "selected text".into(),
    });
    wire_roundtrip(BrowserServerMsg::SelectionChanged {
        text: String::new(),
    });
}

#[test]
fn browser_server_msg_clipboard_write_roundtrips() {
    wire_roundtrip(BrowserServerMsg::ClipboardWrite {
        text: "clipboard content".into(),
    });
}

#[test]
fn browser_server_msg_page_error_roundtrips() {
    wire_roundtrip(BrowserServerMsg::PageError {
        code: -2,
        error_text: "ERR_FAILED".into(),
    });
}

#[test]
fn browser_server_msg_renderer_terminated_roundtrips() {
    wire_roundtrip(BrowserServerMsg::RendererTerminated {
        reason: "OOM".into(),
    });
}

#[test]
fn browser_server_msg_browser_unavailable_all_reasons_roundtrips() {
    wire_roundtrip(BrowserServerMsg::BrowserUnavailable {
        reason: BrowserUnavailableReason::RetryExhausted {
            last_error: "spawn failed".into(),
        },
    });
}

#[test]
fn browser_extra_context_round_trips_extension_role() {
    let ctx = BrowserExtraContext {
        role: BrowserRole::Extension,
        session_id: Some("s1".into()),
        window_id: "w1".into(),
        pane_id: "p1".into(),
        activity_id: "a1".into(),
        extension_name: Some("memo".into()),
    };
    let json = serde_json::to_value(&ctx).unwrap();
    assert_eq!(json["role"], "extension");
    assert_eq!(json["extension_name"], "memo");
    let back: BrowserExtraContext = serde_json::from_value(json).unwrap();
    assert!(matches!(back.role, BrowserRole::Extension));
    assert_eq!(back.extension_name.as_deref(), Some("memo"));
}

#[test]
fn browser_extra_context_browser_role_has_no_extension_name() {
    let ctx = BrowserExtraContext {
        role: BrowserRole::Browser,
        session_id: None,
        window_id: "w1".into(),
        pane_id: "p1".into(),
        activity_id: "a1".into(),
        extension_name: None,
    };
    let json = serde_json::to_value(&ctx).unwrap();
    assert_eq!(json["role"], "browser");
    assert!(json["extension_name"].is_null());
}
