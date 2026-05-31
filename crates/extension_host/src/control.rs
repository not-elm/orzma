//! Control-plane wire protocol: NDJSON `call`/`result`/`error` frames over the
//! per-extension control socket. Pure types + encode/decode — no sockets, no
//! Bevy. The host is the server; the extension (SDK) is the client.

use serde::{Deserialize, Serialize};

/// A decoded control request the Bevy glue acts on. `pane_bits` is the
/// `OZMUX_PANE_ID` entity-bits value; the glue resolves it to an `Entity`.
pub struct ControlRequest {
    /// The requested operation.
    pub op: ControlOp,
    /// Raw entity bits of the invoking pane (decoded by the bridge).
    pub pane_bits: u64,
}

/// The operation a control request carries. `split` is the only op in #2.
pub enum ControlOp {
    /// Split the invoking pane, seeding the new pane with `params.activity`.
    Split(SplitParams),
}

/// Parameters of a `split` control request.
#[derive(Deserialize)]
pub struct SplitParams {
    /// Which side of the target the new pane goes.
    pub side: ControlSide,
    /// Split orientation.
    pub orientation: ControlOrientation,
    /// The activity to seed into the new pane.
    pub activity: ActivitySpec,
}

/// Protocol-side split side (mapped to `ozmux_multiplexer::Side` by the bridge).
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlSide {
    /// Left / top.
    Before,
    /// Right / bottom.
    After,
}

/// Protocol-side orientation (mapped to `ozmux_multiplexer::SplitOrientation`).
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlOrientation {
    /// LHS/RHS split.
    Horizontal,
    /// Top/bottom split.
    Vertical,
}

/// The activity spec carried by a split request.
#[derive(Deserialize)]
pub struct ActivitySpec {
    /// Activity kind discriminator (flattened so `kind` tag is at this level).
    #[serde(flatten)]
    pub kind: ActivityKindSpec,
    /// Optional display name.
    #[serde(default)]
    pub name: Option<String>,
    /// The SDK's client-generated activity id (the key its handlers/channels
    /// are registered under). The bridge addresses `{aid, frame}` envelopes
    /// with this.
    pub activity_id: String,
}

/// Protocol-side activity kind. #2 supports only `extension`.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ActivityKindSpec {
    /// An extension activity served from `html_root`.
    Extension {
        /// Filesystem directory the extension serves (SDK sends `dirname(html)`).
        html_root: String,
    },
}

/// The bridge's verdict for a control request, sent back over the oneshot.
pub enum ControlResponse {
    /// Split succeeded; carries the new entities' bits.
    Ok(SplitReply),
    /// Split failed.
    Err(ControlError),
}

/// Successful split payload.
pub struct SplitReply {
    /// New pane entity bits.
    pub new_pane_id: u64,
    /// New activity entity bits.
    pub new_activity_id: u64,
}

/// A control error (mapped to a wire `error` frame).
pub struct ControlError {
    /// Stable error code (`pane_not_found` / `bad_request` / `internal`).
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

/// A failure to parse a control `call` line.
#[derive(Debug)]
pub enum ControlParseError {
    /// Malformed JSON, unknown op, or bad field (maps to `bad_request`).
    BadRequest(String),
}

/// Parses one NDJSON `call` line into `(id, ControlRequest)`.
pub fn parse_call(line: &str) -> Result<(String, ControlRequest), ControlParseError> {
    let raw: RawCall =
        serde_json::from_str(line).map_err(|e| ControlParseError::BadRequest(e.to_string()))?;
    let pane_bits: u64 = raw
        .pane
        .parse()
        .map_err(|_| ControlParseError::BadRequest(format!("non-numeric pane id: {}", raw.pane)))?;
    let op = match raw.op.as_str() {
        "split" => {
            let params: SplitParams = serde_json::from_value(raw.params)
                .map_err(|e| ControlParseError::BadRequest(e.to_string()))?;
            ControlOp::Split(params)
        }
        other => {
            return Err(ControlParseError::BadRequest(format!(
                "unknown op: {other}"
            )));
        }
    };
    Ok((raw.id, ControlRequest { op, pane_bits }))
}

/// Encodes a `ControlResponse` as one NDJSON line (with trailing `\n`).
pub fn encode_response(id: &str, resp: &ControlResponse) -> String {
    #[derive(Serialize)]
    #[serde(tag = "kind")]
    enum Wire<'a> {
        #[serde(rename = "result")]
        Result { id: &'a str, payload: WirePayload },
        #[serde(rename = "error")]
        Error {
            id: &'a str,
            code: &'a str,
            message: &'a str,
        },
    }
    #[derive(Serialize)]
    struct WirePayload {
        new_pane_id: String,
        new_activity_id: String,
    }
    let wire = match resp {
        ControlResponse::Ok(r) => Wire::Result {
            id,
            payload: WirePayload {
                new_pane_id: r.new_pane_id.to_string(),
                new_activity_id: r.new_activity_id.to_string(),
            },
        },
        ControlResponse::Err(e) => Wire::Error {
            id,
            code: &e.code,
            message: &e.message,
        },
    };
    let mut s = serde_json::to_string(&wire).expect("control response serializes");
    s.push('\n');
    s
}

#[derive(Deserialize)]
struct RawCall {
    id: String,
    op: String,
    pane: String,
    params: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memo_style_split_call() {
        let line = r#"{"kind":"call","id":"abc","op":"split","pane":"4294967297","params":{"side":"after","orientation":"vertical","activity":{"kind":"extension","html_root":"/x/memo","name":null,"activity_id":"aid-123"}}}"#;
        let (id, req) = parse_call(line).expect("parse");
        assert_eq!(id, "abc");
        assert_eq!(req.pane_bits, 4294967297);
        let ControlOp::Split(p) = req.op;
        assert!(matches!(p.side, ControlSide::After));
        assert!(matches!(p.orientation, ControlOrientation::Vertical));
        assert_eq!(p.activity.activity_id, "aid-123");
        let ActivityKindSpec::Extension { html_root } = p.activity.kind;
        assert_eq!(html_root, "/x/memo");
    }

    #[test]
    fn rejects_unknown_op() {
        let line = r#"{"kind":"call","id":"x","op":"teleport","pane":"1","params":{}}"#;
        assert!(matches!(
            parse_call(line),
            Err(ControlParseError::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_non_numeric_pane() {
        let line = r#"{"kind":"call","id":"x","op":"split","pane":"not-a-number","params":{"side":"after","orientation":"vertical","activity":{"kind":"extension","html_root":"/x","name":null,"activity_id":"x"}}}"#;
        assert!(matches!(
            parse_call(line),
            Err(ControlParseError::BadRequest(_))
        ));
    }

    #[test]
    fn encodes_result_and_error_lines() {
        let ok = encode_response(
            "id1",
            &ControlResponse::Ok(SplitReply {
                new_pane_id: 7,
                new_activity_id: 9,
            }),
        );
        assert_eq!(
            ok,
            "{\"kind\":\"result\",\"id\":\"id1\",\"payload\":{\"new_pane_id\":\"7\",\"new_activity_id\":\"9\"}}\n"
        );
        let err = encode_response(
            "id2",
            &ControlResponse::Err(ControlError {
                code: "pane_not_found".into(),
                message: "nope".into(),
            }),
        );
        assert_eq!(
            err,
            "{\"kind\":\"error\",\"id\":\"id2\",\"code\":\"pane_not_found\",\"message\":\"nope\"}\n"
        );
    }
}
