use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
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

#[derive(NewType)]
pub struct PlainNewtype(#[allow(dead_code)] String);

#[test]
fn no_attributes_emits_no_impls() {
    // Compiles. PlainNewtype has neither Display, nor AsRef<str>, nor new().
    // The presence of this struct (and successful compile) is the assertion.
    let _x = PlainNewtype(String::from("hello"));
}

#[test]
fn default_uses_new() {
    let a: SampleId = Default::default();
    let b: SampleId = SampleId::new();
    let a_inner: &str = a.as_ref();
    let b_inner: &str = b.as_ref();
    assert_eq!(a_inner.len(), 36);
    assert_eq!(b_inner.len(), 36);
    assert_ne!(a_inner, b_inner);
}
