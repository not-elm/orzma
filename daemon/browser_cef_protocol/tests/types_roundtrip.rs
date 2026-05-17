use ozmux_browser_cef_protocol::types::{ActivityId, FrameKey, Rect};
use ozmux_browser_cef_protocol::wire::{
    BrowserClientMsg, BrowserProfileWire, BrowserServerMsg, BrowserUnavailableReason, CefCookieDto,
    CursorKind, FrameSubscriptionReply, HostCommand, HostEvent, ImeUnderline, InputEvent,
    KeyEventType, MouseButton, MustRestartReason, SameSite,
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

// --- Wire schema tests (added in PoC Task 12) ---

fn wire_roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(v: T) -> Vec<u8> {
    let bytes = rmp_serde::to_vec_named(&v).expect("serialize");
    let back: T = rmp_serde::from_slice(&bytes).expect("deserialize");
    let bytes2 = rmp_serde::to_vec_named(&back).expect("re-serialize");
    assert_eq!(bytes, bytes2, "roundtrip serialization not deterministic");
    bytes
}

#[test]
fn host_command_browser_create_roundtrips() {
    wire_roundtrip(HostCommand::BrowserCreate {
        aid: ActivityId("a1".into()),
        initial_url: "https://example.com/".into(),
        epoch: 1,
        cookies: vec![],
        profile: BrowserProfileWire::Named {
            name: "default".into(),
        },
    });
}

#[test]
fn host_command_browser_create_with_cookies_roundtrips() {
    // Covers every SameSite variant, both expires_utc Some+None, multi-cookie payload.
    let cookies = vec![
        CefCookieDto {
            url: "https://example.com/".into(),
            name: "session".into(),
            value: "tok123".into(),
            domain: "example.com".into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires_utc: Some(1_700_000_000_000_000.0),
            same_site: SameSite::Lax,
        },
        CefCookieDto {
            url: "https://example.com/api".into(),
            name: "csrf".into(),
            value: "abc".into(),
            domain: "example.com".into(),
            path: "/api".into(),
            secure: true,
            http_only: false,
            expires_utc: None,
            same_site: SameSite::Strict,
        },
        CefCookieDto {
            url: "https://other.example/".into(),
            name: "embed".into(),
            value: "1".into(),
            domain: "other.example".into(),
            path: "/".into(),
            secure: false,
            http_only: false,
            expires_utc: None,
            same_site: SameSite::None,
        },
        CefCookieDto {
            url: "https://example.com/".into(),
            name: "pref".into(),
            value: "dark".into(),
            domain: "example.com".into(),
            path: "/".into(),
            secure: false,
            http_only: false,
            expires_utc: Some(0.0),
            same_site: SameSite::Unspecified,
        },
    ];
    wire_roundtrip(HostCommand::BrowserCreate {
        aid: ActivityId("a1".into()),
        initial_url: "https://example.com/".into(),
        epoch: 1,
        cookies,
        profile: BrowserProfileWire::Named {
            name: "default".into(),
        },
    });
}

#[test]
fn host_command_shutdown_roundtrips() {
    wire_roundtrip(HostCommand::Shutdown);
}

// --- HostCommand new variants (Task A15) ---

#[test]
fn host_command_recreate_shm_roundtrips() {
    wire_roundtrip(HostCommand::RecreateShm {
        aid: ActivityId("a2".into()),
        new_epoch: 7,
    });
}

#[test]
fn host_command_navigate_roundtrips() {
    wire_roundtrip(HostCommand::Navigate {
        aid: ActivityId("a1".into()),
        url: "https://example.com/page".into(),
    });
}

#[test]
fn host_command_navigate_history_roundtrips() {
    wire_roundtrip(HostCommand::NavigateHistory {
        aid: ActivityId("a1".into()),
        delta: -1,
    });
    wire_roundtrip(HostCommand::NavigateHistory {
        aid: ActivityId("a1".into()),
        delta: 1,
    });
}

#[test]
fn host_command_send_input_mouse_roundtrips() {
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::MouseMove {
            x: 100,
            y: 200,
            modifiers: 0,
        },
    });
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::MouseClick {
            x: 50,
            y: 60,
            button: MouseButton::Left,
            count: 1,
            mouse_up: false,
            modifiers: 0,
        },
    });
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::MouseWheel {
            x: 200,
            y: 300,
            delta_x: 0,
            delta_y: -120,
            modifiers: 0,
        },
    });
    // All three button variants
    for btn in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
        wire_roundtrip(HostCommand::SendInput {
            aid: ActivityId("a1".into()),
            input: InputEvent::MouseClick {
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
fn host_command_send_input_key_roundtrips() {
    for event_type in [
        KeyEventType::RawKeyDown,
        KeyEventType::KeyUp,
        KeyEventType::Char,
    ] {
        wire_roundtrip(HostCommand::SendInput {
            aid: ActivityId("a1".into()),
            input: InputEvent::Key {
                event_type,
                windows_key_code: 65,
                native_key_code: 30,
                modifiers: 0,
                character: b'a' as u16,
                unmodified_character: b'a' as u16,
                focus_on_editable_field: true,
            },
        });
    }
}

#[test]
fn host_command_send_input_ime_set_composition_roundtrips() {
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::ImeSetComposition {
            text: "hello".into(),
            underlines: vec![ImeUnderline {
                from: 0,
                to: 5,
                color: 0xFF000000,
                background_color: 0x00000000,
                thick: false,
            }],
            replacement_range: (-1, -1),
            selection_range: (5, 5),
        },
    });
    // Empty underlines, custom replacement_range
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::ImeSetComposition {
            text: "abc".into(),
            underlines: vec![],
            replacement_range: (2, 4),
            selection_range: (3, 3),
        },
    });
}

#[test]
fn host_command_send_input_ime_commit_roundtrips() {
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::ImeCommit {
            text: "confirmed".into(),
            replacement_range: None,
            relative_cursor_pos: 0,
        },
    });
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::ImeCommit {
            text: "xy".into(),
            replacement_range: Some((1, 3)),
            relative_cursor_pos: -1,
        },
    });
}

#[test]
fn host_command_send_input_ime_cancel_roundtrips() {
    wire_roundtrip(HostCommand::SendInput {
        aid: ActivityId("a1".into()),
        input: InputEvent::ImeCancel,
    });
}

#[test]
fn host_command_pause_resume_roundtrips() {
    wire_roundtrip(HostCommand::PauseScreencast {
        aid: ActivityId("a1".into()),
    });
    wire_roundtrip(HostCommand::ResumeScreencast {
        aid: ActivityId("a1".into()),
    });
}

#[test]
fn host_command_get_selection_roundtrips() {
    wire_roundtrip(HostCommand::GetSelection {
        aid: ActivityId("a1".into()),
        request_id: 99,
    });
}

#[test]
fn host_command_set_clipboard_roundtrips() {
    wire_roundtrip(HostCommand::SetClipboard {
        text: "hello clipboard".into(),
    });
}

// --- HostEvent roundtrips ---

#[test]
fn host_event_frame_descriptor_roundtrips() {
    wire_roundtrip(HostEvent::FrameDescriptor {
        aid: ActivityId("a1".into()),
        lap: 100,
        slot_idx: 2,
        frame_seq: 100,
        captured_at_us: 1_700_000_000_000_000,
        is_keyframe: true,
        damage_rects: vec![Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 800,
        }],
        is_popup: false,
    });
}

#[test]
fn host_event_frame_descriptor_with_popup_roundtrips() {
    wire_roundtrip(HostEvent::FrameDescriptor {
        aid: ActivityId("a1".into()),
        lap: 42,
        slot_idx: 0,
        frame_seq: 10,
        captured_at_us: 1_000_000,
        is_keyframe: false,
        damage_rects: vec![],
        is_popup: true,
    });
}

#[test]
fn host_event_nav_state_changed_roundtrips() {
    wire_roundtrip(HostEvent::NavStateChanged {
        aid: ActivityId("a1".into()),
        url: "https://example.com/page2".into(),
        title: "Page 2".into(),
        can_back: true,
        can_forward: false,
    });
}

#[test]
fn host_event_title_changed_roundtrips() {
    wire_roundtrip(HostEvent::TitleChanged {
        aid: ActivityId("a1".into()),
        title: "New Title".into(),
    });
}

#[test]
fn host_event_cursor_changed_roundtrips() {
    wire_roundtrip(HostEvent::CursorChanged {
        aid: ActivityId("a1".into()),
        cursor: CursorKind::Pointer,
    });
}

#[test]
fn host_event_selection_changed_roundtrips() {
    wire_roundtrip(HostEvent::SelectionChanged {
        aid: ActivityId("a1".into()),
        text: "selected text".into(),
    });
    // Empty selection
    wire_roundtrip(HostEvent::SelectionChanged {
        aid: ActivityId("a1".into()),
        text: String::new(),
    });
}

#[test]
fn host_event_page_error_roundtrips() {
    wire_roundtrip(HostEvent::PageError {
        aid: ActivityId("a1".into()),
        code: -6,
        error_text: "ERR_CONNECTION_REFUSED".into(),
    });
}

#[test]
fn host_event_render_process_terminated_roundtrips() {
    wire_roundtrip(HostEvent::RenderProcessTerminated {
        aid: ActivityId("a1".into()),
        reason: "KILLED".into(),
    });
}

#[test]
fn host_event_log_line_roundtrips() {
    wire_roundtrip(HostEvent::LogLine {
        level: "WARNING".into(),
        text: "something suspicious".into(),
    });
}

#[test]
fn host_event_crashed_roundtrips() {
    wire_roundtrip(HostEvent::Crashed {
        reason: "SIGSEGV in renderer".into(),
    });
}

// --- BrowserClientMsg new variants (Task A15) ---

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

// --- BrowserServerMsg new variants (Task A15) ---

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
    use std::path::PathBuf;
    wire_roundtrip(BrowserServerMsg::BrowserUnavailable {
        reason: BrowserUnavailableReason::RetryExhausted {
            last_error: "spawn failed".into(),
        },
    });
    wire_roundtrip(BrowserServerMsg::BrowserUnavailable {
        reason: BrowserUnavailableReason::BinaryNotFound {
            path: PathBuf::from("/usr/local/bin/cef_host"),
        },
    });
    wire_roundtrip(BrowserServerMsg::BrowserUnavailable {
        reason: BrowserUnavailableReason::CefInitFailed { exit_code: 1 },
    });
    wire_roundtrip(BrowserServerMsg::BrowserUnavailable {
        reason: BrowserUnavailableReason::ProtocolMismatch {
            expected: 4,
            got: 3,
        },
    });
}

#[test]
fn host_command_browser_create_roundtrips_with_named_profile() {
    wire_roundtrip(HostCommand::BrowserCreate {
        aid: ActivityId("a1".into()),
        initial_url: "https://example.com".into(),
        epoch: 1,
        cookies: vec![],
        profile: BrowserProfileWire::Named {
            name: "work".into(),
        },
    });
}

#[test]
fn host_command_browser_create_roundtrips_incognito() {
    wire_roundtrip(HostCommand::BrowserCreate {
        aid: ActivityId("a2".into()),
        initial_url: "about:blank".into(),
        epoch: 1,
        cookies: vec![],
        profile: BrowserProfileWire::Incognito,
    });
}
