//! JSON-RPC 2.0 envelopes the LSP base protocol rides on.
//!
//! Outgoing: [`Request`] (carries an id, expects a response) and
//! [`Notification`] (no id, fire-and-forget). Incoming: [`Response`] (success
//! or error keyed by request id) or a server-initiated request / notification
//! we receive as a raw [`serde_json::Value`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Marker for the JSON-RPC version field, always `"2.0"` on the wire.
pub const JSONRPC_VERSION: &str = "2.0";

/// Outgoing JSON-RPC request — has an `id`, expects exactly one [`Response`].
#[derive(Debug, Serialize)]
pub struct Request<'a, P: Serialize> {
    pub jsonrpc: &'static str,
    pub id: i64,
    pub method: &'a str,
    pub params: P,
}

impl<'a, P: Serialize> Request<'a, P> {
    /// Build a request envelope. The constructor stamps the version so
    /// callers only specify id, method, and params.
    pub fn new(id: i64, method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            method,
            params,
        }
    }
}

/// Outgoing JSON-RPC notification — no id, no response.
#[derive(Debug, Serialize)]
pub struct Notification<'a, P: Serialize> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: P,
}

impl<'a, P: Serialize> Notification<'a, P> {
    /// Build a notification envelope.
    pub fn new(method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            method,
            params,
        }
    }
}

/// A response received from the server: either `result` or `error` is set.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<i64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

/// JSON-RPC error payload.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// JSON-RPC parsing/serialisation error.
#[derive(Debug, Error)]
pub enum JsonRpcError {
    #[error("failed to serialise outgoing message: {0}")]
    Serialise(#[source] serde_json::Error),
    #[error("failed to deserialise incoming message: {0}")]
    Deserialise(#[source] serde_json::Error),
}

/// Serialise a request to JSON bytes ready for the codec.
pub fn encode_request<P: Serialize>(request: &Request<'_, P>) -> Result<Vec<u8>, JsonRpcError> {
    serde_json::to_vec(request).map_err(JsonRpcError::Serialise)
}

/// Serialise a notification to JSON bytes ready for the codec.
pub fn encode_notification<P: Serialize>(
    notification: &Notification<'_, P>,
) -> Result<Vec<u8>, JsonRpcError> {
    serde_json::to_vec(notification).map_err(JsonRpcError::Serialise)
}

/// Parse a response from JSON bytes received via the codec.
pub fn decode_response(bytes: &[u8]) -> Result<Response, JsonRpcError> {
    serde_json::from_slice(bytes).map_err(JsonRpcError::Deserialise)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialises_with_version_and_fields() {
        let request = Request::new(
            1,
            "initialize",
            serde_json::json!({"clientName": "cockpit"}),
        );
        let bytes = encode_request(&request).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 1);
        assert_eq!(value["method"], "initialize");
        assert_eq!(value["params"]["clientName"], "cockpit");
    }

    #[test]
    fn notification_omits_id() {
        let notification = Notification::new("initialized", serde_json::json!({}));
        let bytes = encode_notification(&notification).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "initialized");
        assert!(
            value.get("id").is_none(),
            "notifications must not carry an id"
        );
    }

    #[test]
    fn response_parses_a_success_payload() {
        let bytes = br#"{"jsonrpc":"2.0","id":2,"result":{"capabilities":{}}}"#;
        let response = decode_response(bytes).unwrap();
        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, Some(2));
        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[test]
    fn response_parses_an_error_payload() {
        let bytes =
            br#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"Method not found"}}"#;
        let response = decode_response(bytes).unwrap();
        assert_eq!(response.id, Some(3));
        assert!(response.result.is_none());
        let err = response.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn malformed_response_yields_a_typed_error() {
        let err = decode_response(b"not json").unwrap_err();
        assert!(matches!(err, JsonRpcError::Deserialise(_)));
    }
}
