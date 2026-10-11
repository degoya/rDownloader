//! The JSON-RPC 2.0 envelope as aria2 speaks it: one call or a batch, the `token:` first
//! parameter, and the answers in aria2's shape.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// The prefix aria2 expects on its RPC secret, sent as the first parameter of every call.
const TOKEN_PREFIX: &str = "token:";

/// One method call, its secret already taken off the parameters.
#[derive(Debug)]
pub(super) struct Call {
    /// Echoed as sent; `null` when the call carried none.
    pub id: Value,
    pub method: String,
    pub params: Vec<Value>,
    /// The `token:` parameter without its prefix, when the call carried one.
    pub token: Option<String>,
}

/// What one HTTP request carried.
#[derive(Debug)]
pub(super) enum Request {
    Single(Call),
    /// A JSON-RPC batch. An element that is no call answers its own error in its slot, as
    /// JSON-RPC asks, while the others run.
    Batch(Vec<Result<Call, Failure>>),
}

/// A failed call: aria2's error code and message, and the call's id.
#[derive(Debug)]
pub(super) struct Failure {
    pub id: Value,
    pub error: RpcError,
}

/// An error object of the protocol.
///
/// aria2 answers nearly every refusal with code `1` and a message; only the envelope's own
/// faults and a method this subset does not have carry the JSON-RPC codes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }

    fn parse() -> Self {
        Self {
            code: -32700,
            message: "Parse error.".to_owned(),
        }
    }

    fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "Invalid Request.".to_owned(),
        }
    }

    fn invalid_params() -> Self {
        Self {
            code: -32602,
            message: "Invalid params.".to_owned(),
        }
    }

    /// A method this subset does not answer: JSON-RPC's `Method not found`, so a front end
    /// tells "not supported here" from a call that failed (RD-1240-28).
    pub(super) fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
        }
    }

    /// aria2's own word for a wrong or missing secret, which the front ends look for.
    pub(super) fn unauthorized() -> Self {
        Self::new("Unauthorized")
    }

    pub(super) fn to_value(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

/// Reads a request body. An unreadable one fails whole: there is no call to run.
pub(super) fn parse(body: &[u8]) -> Result<Request, Failure> {
    let refused = |error| Failure {
        id: Value::Null,
        error,
    };
    let value: Value = serde_json::from_slice(body).map_err(|_| refused(RpcError::parse()))?;
    match value {
        Value::Array(items) if !items.is_empty() => {
            Ok(Request::Batch(items.into_iter().map(call).collect()))
        }
        Value::Object(_) => call(value).map(Request::Single),
        _ => Err(refused(RpcError::invalid_request())),
    }
}

/// One call object, its secret split off.
fn call(value: Value) -> Result<Call, Failure> {
    let Value::Object(mut object) = value else {
        return Err(Failure {
            id: Value::Null,
            error: RpcError::invalid_request(),
        });
    };
    let id = object.remove("id").unwrap_or(Value::Null);
    let Some(Value::String(method)) = object.remove("method") else {
        return Err(Failure {
            id,
            error: RpcError::invalid_request(),
        });
    };
    let params = match object.remove("params") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(params)) => params,
        Some(_) => {
            return Err(Failure {
                id,
                error: RpcError::invalid_params(),
            });
        }
    };
    let (token, params) = split_token(params);
    Ok(Call {
        id,
        method,
        params,
        token,
    })
}

/// Takes a leading `token:<secret>` off `params`, as aria2 does before a method sees them.
pub(super) fn split_token(mut params: Vec<Value>) -> (Option<String>, Vec<Value>) {
    let token = match params.first() {
        Some(Value::String(first)) => first.strip_prefix(TOKEN_PREFIX).map(str::to_owned),
        _ => None,
    };
    if token.is_some() {
        params.remove(0);
    }
    (token, params)
}

/// The one secret every call of a request carries, or `None` when one lacks it or two differ.
///
/// A `system.multicall` carries no secret of its own -- aria2 checks each call inside it -- so
/// its inner calls count instead. One credential per HTTP request keeps the check, its audit
/// record and the token's call limit at one each however many calls a front end batches; a
/// request that mixes secrets is no front end's and is refused whole.
pub(super) fn request_token(request: &Request) -> Option<String> {
    let mut tokens: Vec<Option<String>> = Vec::new();
    let mut note = |call: &Call| {
        if call.method == "system.multicall" {
            for entry in multicall_entries(&call.params).unwrap_or_default() {
                tokens.push(entry.and_then(|(_, token, _)| token));
            }
        } else {
            tokens.push(call.token.clone());
        }
    };
    match request {
        Request::Single(call) => note(call),
        Request::Batch(items) => items.iter().flatten().for_each(&mut note),
    }
    let first = tokens.first().cloned().flatten()?;
    tokens
        .iter()
        .all(|token| token.as_deref() == Some(first.as_str()))
        .then_some(first)
}

/// One call inside a `system.multicall`: its method, its secret and its parameters.
pub(super) type MulticallEntry = (String, Option<String>, Vec<Value>);

/// The calls inside a `system.multicall`, or `None` for an element that is no call. `None`
/// overall when the parameter is not a list of them.
pub(super) fn multicall_entries(params: &[Value]) -> Option<Vec<Option<MulticallEntry>>> {
    let Some(Value::Array(entries)) = params.first() else {
        return None;
    };
    Some(
        entries
            .iter()
            .map(|entry| {
                let method = entry.get("methodName")?.as_str()?.to_owned();
                let params = match entry.get("params") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(params)) => params.clone(),
                    Some(_) => return None,
                };
                let (token, params) = split_token(params);
                Some((method, token, params))
            })
            .collect(),
    )
}

/// The answer object of one call.
pub(super) fn answer(id: Value, outcome: &Result<Value, RpcError>) -> Value {
    match outcome {
        Ok(result) => json!({ "id": id, "jsonrpc": "2.0", "result": result }),
        Err(error) => json!({ "id": id, "jsonrpc": "2.0", "error": error.to_value() }),
    }
}

/// One call's answer with aria2's status codes: `200` for a result, `400` for a malformed
/// envelope or a method this subset does not have, and `500` for a method that failed -- the
/// front ends read the error object either way. A missing method is no `404`: that is the
/// answer of a switched-off adapter, and no `500`: nothing failed on this side.
pub(super) fn single(id: Value, outcome: &Result<Value, RpcError>) -> Response {
    let status = match outcome {
        Ok(_) => StatusCode::OK,
        Err(error) if matches!(error.code, -32700 | -32600 | -32601 | -32602) => {
            StatusCode::BAD_REQUEST
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, axum::Json(answer(id, outcome))).into_response()
}

/// The refusal of a request whose secret did not pass: `403`, with aria2's `Unauthorized` in
/// each call's slot so a front end shows its own "wrong secret" message.
pub(super) fn unauthorized(request: &Request) -> Response {
    let refused = Err(RpcError::unauthorized());
    let body = match request {
        Request::Single(call) => answer(call.id.clone(), &refused),
        Request::Batch(items) => Value::Array(
            items
                .iter()
                .map(|item| match item {
                    Ok(call) => answer(call.id.clone(), &refused),
                    Err(failure) => answer(failure.id.clone(), &Err(failure.error.clone())),
                })
                .collect(),
        ),
    };
    (StatusCode::FORBIDDEN, axum::Json(body)).into_response()
}
