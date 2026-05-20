//! CEF process-message names and JSON payloads for the ozmux V8 binding
//! bridge. Used by both render-side (sending call.request / sub.open /
//! sub.cancel) and browser-side (sending call.response / sub.event).
//!
//! Process messages cross the CEF render↔browser boundary via
//! `Frame::send_process_message`. Each message carries a single JSON
//! payload stringified into the first argument of the CEF `ListValue`. The
//! payload shape matches the SDK protocol described in
//! `sdk/typescript/src/server/protocol.ts` (HandlerCallFrame, SubOpenFrame,
//! etc.) so the browser-side bridge can re-serialize them onto the
//! extension UDS without re-mapping field names.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Render → browser: invoke a one-shot extension handler.
pub(crate) const MSG_CALL_REQUEST: &str = "ozmux.call.request";

/// Browser → render: result or error for a previous `MSG_CALL_REQUEST`.
pub(crate) const MSG_CALL_RESPONSE: &str = "ozmux.call.response";

/// Render → browser: open a long-lived subscription.
pub(crate) const MSG_SUB_OPEN: &str = "ozmux.sub.open";

/// Render → browser: cancel an active subscription by id.
pub(crate) const MSG_SUB_CANCEL: &str = "ozmux.sub.cancel";

/// Browser → render: a `sub.data` / `sub.complete` / `sub.error` event.
pub(crate) const MSG_SUB_EVENT: &str = "ozmux.sub.event";

// NOTE: CallRequest/SubOpen/SubCancel are the render→browser payloads. The
// browser-side `OzmuxClient::on_process_message_received` does not deserialize
// them into these structs — it forwards the raw JSON string straight onto the
// extension UDS after a kind-field re-stamp. The types are kept here so the
// V8-side encoder (Task 7c) can build payloads from the same definitions.
#[allow(dead_code, reason = "render-side payload types consumed by V8 binding in Task 7c")]
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CallRequest {
    pub id: String,
    pub name: String,
    pub payload: Value,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CallResponse {
    Result {
        id: String,
        payload: Value,
    },
    Error {
        id: String,
        code: String,
        message: String,
    },
}

#[allow(dead_code, reason = "render-side payload types consumed by V8 binding in Task 7c")]
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct SubOpen {
    pub id: String,
    pub name: String,
    pub params: Value,
}

#[allow(dead_code, reason = "render-side payload types consumed by V8 binding in Task 7c")]
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct SubCancel {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub(crate) enum SubEvent {
    #[serde(rename = "sub.data")]
    Data { id: String, payload: Value },
    #[serde(rename = "sub.complete")]
    Complete { id: String },
    #[serde(rename = "sub.error")]
    Error {
        id: String,
        code: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn call_request_round_trips() {
        let req = CallRequest {
            id: "c1".to_string(),
            name: "greet".to_string(),
            payload: json!({"who": "world"}),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: CallRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn call_response_result_round_trips() {
        let v = CallResponse::Result {
            id: "c1".to_string(),
            payload: json!({"ok": true}),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains(r#""kind":"result""#));
        let back: CallResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn call_response_error_round_trips() {
        let v = CallResponse::Error {
            id: "c1".to_string(),
            code: "EBAD".to_string(),
            message: "nope".to_string(),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains(r#""kind":"error""#));
        let back: CallResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn sub_open_round_trips() {
        let v = SubOpen {
            id: "s1".to_string(),
            name: "counter".to_string(),
            params: json!({"max": 5}),
        };
        let s = serde_json::to_string(&v).unwrap();
        let back: SubOpen = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn sub_cancel_round_trips() {
        let v = SubCancel {
            id: "s1".to_string(),
        };
        let s = serde_json::to_string(&v).unwrap();
        let back: SubCancel = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn sub_event_data_round_trips() {
        let v = SubEvent::Data {
            id: "s1".to_string(),
            payload: json!({"n": 1}),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains(r#""kind":"sub.data""#));
        let back: SubEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn sub_event_complete_round_trips() {
        let v = SubEvent::Complete {
            id: "s1".to_string(),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains(r#""kind":"sub.complete""#));
        let back: SubEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn sub_event_error_round_trips() {
        let v = SubEvent::Error {
            id: "s1".to_string(),
            code: "EBAD".to_string(),
            message: "boom".to_string(),
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains(r#""kind":"sub.error""#));
        let back: SubEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }
}
