//! RPC handler boxing and tuple-param dispatch.

use crate::error::RpcError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;

/// A type-erased RPC handler: args array in, JSON value or error out.
pub(crate) type BoxedHandler =
    Arc<dyn Fn(Vec<Value>) -> Result<Value, RpcError> + Send + Sync + 'static>;

/// Boxes a typed handler whose parameter is a tuple deserialized from the
/// positional args array, returning a `BoxedHandler`.
pub(crate) fn make_handler<P, R, F>(f: F) -> BoxedHandler
where
    P: DeserializeOwned,
    R: Serialize,
    F: Fn(P) -> Result<R, RpcError> + Send + Sync + 'static,
{
    Arc::new(move |args: Vec<Value>| {
        // NOTE: serde_json deserializes () from null, not from an empty array, so
        // an empty args list must be presented as Value::Null to satisfy unit params.
        let raw = if args.is_empty() {
            Value::Null
        } else {
            Value::Array(args)
        };
        let params: P = serde_json::from_value(raw)?;
        let result = f(params)?;
        Ok(serde_json::to_value(result)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn single_arg_tuple_dispatch() {
        let h = make_handler(|(name,): (String,)| Ok(format!("hi {name}")));
        let out = h(vec![json!("ada")]).unwrap();
        assert_eq!(out, json!("hi ada"));
    }

    #[test]
    fn two_arg_tuple_dispatch() {
        let h = make_handler(|(a, b): (u32, u32)| Ok(a + b));
        assert_eq!(h(vec![json!(2), json!(3)]).unwrap(), json!(5));
    }

    #[test]
    fn unit_arg_dispatch() {
        let h = make_handler(|(): ()| Ok("ok"));
        assert_eq!(h(vec![]).unwrap(), json!("ok"));
    }

    #[test]
    fn bad_args_become_rpc_error() {
        let h = make_handler(|(_n,): (u32,)| Ok(0u32));
        assert!(h(vec![json!("not a number")]).is_err());
    }
}
