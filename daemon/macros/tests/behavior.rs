use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
pub struct SampleId(String);

#[test]
fn new_returns_uuid_v4_string() {
    let id = SampleId::new();
    let inner: &str = id.as_ref();
    // UUID v4 string is 36 chars: 8-4-4-4-12 with hyphens
    assert_eq!(inner.len(), 36);
    assert_eq!(inner.matches('-').count(), 4);
}

#[test]
fn two_news_produce_different_ids() {
    let a = SampleId::new();
    let b = SampleId::new();
    assert_ne!(a, b);
}

#[test]
fn display_matches_inner_string() {
    let id = SampleId::new();
    let s: String = format!("{}", id);
    assert_eq!(s.as_str(), <SampleId as AsRef<str>>::as_ref(&id));
}

#[test]
fn serde_round_trip_is_string() {
    let id = SampleId::new();
    let json = serde_json::to_string(&id).unwrap();
    assert!(json.starts_with('"') && json.ends_with('"'));
    let back: SampleId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}
