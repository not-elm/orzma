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

/// The operation a control request carries.
pub enum ControlOp {
    /// Split the invoking pane, seeding the new pane with `params.activity`.
    Split(SplitParams),
    /// Add an activity (tab) to the invoking pane without splitting.
    AddActivity(AddActivityParams),
    /// Make `activity_id` the invoking pane's active activity.
    Activate(ActivateParams),
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

/// Parameters of an `add_activity` control request.
#[derive(Deserialize)]
pub struct AddActivityParams {
    /// The activity to add to the invoking pane.
    pub activity: ActivitySpec,
}

/// Parameters of an `activate` control request.
#[derive(Deserialize)]
pub struct ActivateParams {
    /// The SDK activity id (entity bits, decimal string) to activate.
    pub activity_id: String,
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

/// The activity spec carried by a split or add_activity request.
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

/// Protocol-side activity kind.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ActivityKindSpec {
    /// An extension activity whose webview loads `entry` (the client's HTML path
    /// relative to the extension dir / asset root).
    Extension {
        /// HTML entry path relative to the extension dir (e.g. `index.html`,
        /// `ui/app.html`); the `ozmux-ext://<name>/<entry>` URL path.
        entry: String,
        /// Owning extension name (so the host can route per extension). Optional
        /// for back-compat; absent on older SDK payloads.
        #[serde(default)]
        extension_name: Option<String>,
    },
    /// An embedded browser activity. `url` is the raw `@browser open` input
    /// (a URL or search words), passed through verbatim for the host to resolve.
    Browser {
        /// Raw user input (a URL or search words).
        url: String,
    },
}

/// Successful reply payload per op.
pub enum ControlReply {
    /// Split succeeded; carries the new entities' bits.
    Split {
        new_pane_id: u64,
        new_activity_id: u64,
    },
    /// Add-activity succeeded; carries the new activity entity bits.
    AddActivity { new_activity_id: u64 },
    /// Activate succeeded (no payload).
    Activate,
}

/// The bridge's verdict for a control request, sent back over the oneshot.
pub enum ControlResponse {
    /// Operation succeeded.
    Ok(ControlReply),
    /// Operation failed.
    Err(ControlError),
}

/// A control error (mapped to a wire `error` frame).
pub struct ControlError {
    /// Stable error code (`pane_not_found` / `bad_request` / `internal`).
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

/// A failure to parse a control `call` line.
#[derive(Debug, thiserror::Error)]
pub enum ControlParseError {
    /// Malformed JSON, unknown op, or bad field (maps to `bad_request`).
    #[error("{0}")]
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
        "add_activity" => {
            let params: AddActivityParams = serde_json::from_value(raw.params)
                .map_err(|e| ControlParseError::BadRequest(e.to_string()))?;
            ControlOp::AddActivity(params)
        }
        "activate" => {
            let params: ActivateParams = serde_json::from_value(raw.params)
                .map_err(|e| ControlParseError::BadRequest(e.to_string()))?;
            ControlOp::Activate(params)
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
    #[serde(untagged)]
    enum Payload {
        Split {
            new_pane_id: String,
            new_activity_id: String,
        },
        AddActivity {
            new_activity_id: String,
        },
        Empty {},
    }
    #[derive(Serialize)]
    #[serde(tag = "kind")]
    enum Wire<'a> {
        #[serde(rename = "result")]
        Result { id: &'a str, payload: Payload },
        #[serde(rename = "error")]
        Error {
            id: &'a str,
            code: &'a str,
            message: &'a str,
        },
    }
    let wire = match resp {
        ControlResponse::Ok(ControlReply::Split {
            new_pane_id,
            new_activity_id,
        }) => Wire::Result {
            id,
            payload: Payload::Split {
                new_pane_id: new_pane_id.to_string(),
                new_activity_id: new_activity_id.to_string(),
            },
        },
        ControlResponse::Ok(ControlReply::AddActivity { new_activity_id }) => Wire::Result {
            id,
            payload: Payload::AddActivity {
                new_activity_id: new_activity_id.to_string(),
            },
        },
        ControlResponse::Ok(ControlReply::Activate) => Wire::Result {
            id,
            payload: Payload::Empty {},
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
        let line = r#"{"kind":"call","id":"abc","op":"split","pane":"4294967297","params":{"side":"after","orientation":"vertical","activity":{"kind":"extension","entry":"index.html","extension_name":"memo","name":null,"activity_id":"aid-123"}}}"#;
        let (id, req) = parse_call(line).expect("parse");
        assert_eq!(id, "abc");
        assert_eq!(req.pane_bits, 4294967297);
        let ControlOp::Split(p) = req.op else {
            panic!("expected Split")
        };
        assert!(matches!(p.side, ControlSide::After));
        assert!(matches!(p.orientation, ControlOrientation::Vertical));
        assert_eq!(p.activity.activity_id, "aid-123");
        let ActivityKindSpec::Extension {
            entry,
            extension_name,
        } = p.activity.kind
        else {
            panic!("expected Extension kind");
        };
        assert_eq!(entry, "index.html");
        assert_eq!(extension_name.as_deref(), Some("memo"));
    }

    #[test]
    fn parses_split_without_extension_name() {
        let line = r#"{"kind":"call","id":"xyz","op":"split","pane":"1","params":{"side":"before","orientation":"horizontal","activity":{"kind":"extension","entry":"index.html","name":null,"activity_id":"aid-456"}}}"#;
        let (id, req) = parse_call(line).expect("parse");
        assert_eq!(id, "xyz");
        let ControlOp::Split(p) = req.op else {
            panic!("expected Split")
        };
        let ActivityKindSpec::Extension {
            entry,
            extension_name,
        } = p.activity.kind
        else {
            panic!("expected Extension kind");
        };
        assert_eq!(entry, "index.html");
        assert_eq!(extension_name, None);
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
        let line = r#"{"kind":"call","id":"x","op":"split","pane":"not-a-number","params":{"side":"after","orientation":"vertical","activity":{"kind":"extension","entry":"index.html","name":null,"activity_id":"x"}}}"#;
        assert!(matches!(
            parse_call(line),
            Err(ControlParseError::BadRequest(_))
        ));
    }

    #[test]
    fn parses_browser_split_call() {
        let line = r#"{"kind":"call","id":"b1","op":"split","pane":"1","params":{"side":"after","orientation":"vertical","activity":{"kind":"browser","url":"github.com","name":null,"activity_id":"aid-b"}}}"#;
        let (id, req) = parse_call(line).expect("parse");
        assert_eq!(id, "b1");
        let ControlOp::Split(p) = req.op else {
            panic!("expected Split");
        };
        assert_eq!(p.activity.activity_id, "aid-b");
        let ActivityKindSpec::Browser { url } = p.activity.kind else {
            panic!("expected Browser kind");
        };
        assert_eq!(url, "github.com");
    }

    #[test]
    fn encodes_result_and_error_lines() {
        let ok = encode_response(
            "id1",
            &ControlResponse::Ok(ControlReply::Split {
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

    #[test]
    fn parses_add_activity_call() {
        let line = r#"{"kind":"call","id":"a1","op":"add_activity","pane":"4294967297","params":{"activity":{"kind":"extension","entry":"index.html","extension_name":"md","name":"x.md","activity_id":"aid-1"}}}"#;
        let (id, req) = parse_call(line).expect("parse");
        assert_eq!(id, "a1");
        let ControlOp::AddActivity(p) = req.op else {
            panic!("expected AddActivity")
        };
        assert_eq!(p.activity.activity_id, "aid-1");
    }

    #[test]
    fn parses_activate_call() {
        let line = r#"{"kind":"call","id":"a2","op":"activate","pane":"1","params":{"activity_id":"aid-9"}}"#;
        let (id, req) = parse_call(line).expect("parse");
        assert_eq!(id, "a2");
        let ControlOp::Activate(p) = req.op else {
            panic!("expected Activate")
        };
        assert_eq!(p.activity_id, "aid-9");
    }

    #[test]
    fn encodes_add_activity_and_activate_replies() {
        let add = encode_response(
            "i1",
            &ControlResponse::Ok(ControlReply::AddActivity { new_activity_id: 7 }),
        );
        assert_eq!(
            add,
            "{\"kind\":\"result\",\"id\":\"i1\",\"payload\":{\"new_activity_id\":\"7\"}}\n"
        );
        let act = encode_response("i2", &ControlResponse::Ok(ControlReply::Activate));
        assert_eq!(act, "{\"kind\":\"result\",\"id\":\"i2\",\"payload\":{}}\n");
    }
}
