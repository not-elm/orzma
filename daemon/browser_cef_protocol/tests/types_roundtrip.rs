use ozmux_browser_cef_protocol::types::{ActivityId, FrameKey, Rect};
use ozmux_browser_cef_protocol::wire::{
    BrowserClientMsg, BrowserServerMsg, FrameSubscriptionReply, HostCommand, HostEvent,
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
    });
}

#[test]
fn host_command_shutdown_roundtrips() {
    wire_roundtrip(HostCommand::Shutdown);
}

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
