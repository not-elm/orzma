use ozmux_browser_cef_protocol::types::{ActivityId, FrameKey, Rect};

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
