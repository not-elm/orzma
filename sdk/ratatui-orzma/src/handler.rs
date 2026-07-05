//! RPC handler boxing and single-params dispatch.

use crate::error::RpcError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;

/// A type-erased RPC handler: the params value in, JSON value or error out.
pub(crate) type BoxedHandler =
    Arc<dyn Fn(Value) -> Result<Value, RpcError> + Send + Sync + 'static>;

/// Boxes a typed handler whose parameter is any `DeserializeOwned` type,
/// deserialized from the single `params` value, returning a `BoxedHandler`.
pub(crate) fn make_handler<P, R, F>(f: F) -> BoxedHandler
where
    P: DeserializeOwned,
    R: Serialize,
    F: Fn(P) -> Result<R, RpcError> + Send + Sync + 'static,
{
    Arc::new(move |params: Value| {
        let p: P = serde_json::from_value(params)?;
        let result = f(p)?;
        Ok(serde_json::to_value(result)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(serde::Deserialize)]
    struct SaveReq {
        id: u32,
        name: String,
    }

    #[test]
    fn scalar_params_dispatch() {
        let h = make_handler(|name: String| Ok(format!("hi {name}")));
        let out = h(json!("ada")).unwrap();
        assert_eq!(out, json!("hi ada"));
    }

    #[test]
    fn struct_params_dispatch() {
        let h = make_handler(|req: SaveReq| Ok(format!("{}:{}", req.id, req.name)));
        assert_eq!(h(json!({"id": 7, "name": "ada"})).unwrap(), json!("7:ada"));
    }

    #[test]
    fn tuple_params_dispatch() {
        let h = make_handler(|(a, b): (u32, u32)| Ok(a + b));
        assert_eq!(h(json!([2, 3])).unwrap(), json!(5));
    }

    #[test]
    fn unit_params_dispatch() {
        let h = make_handler(|_: ()| Ok("ok"));
        assert_eq!(h(Value::Null).unwrap(), json!("ok"));
    }

    #[test]
    fn bad_params_become_rpc_error() {
        let h = make_handler(|_n: u32| Ok(0u32));
        assert!(h(json!("not a number")).is_err());
    }
}
