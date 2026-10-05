//! Requests against the router, and reading back what it answered.
//!
//! One place for the transport: every helper below ends in [`send_raw`], and every body is
//! read by [`body_bytes`]. The copies these replaced differed in nothing but whether a body
//! that is not JSON reads as `null` or fails the test — which is [`send`] against
//! [`send_strict`] now, chosen by name at the call site.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header, request::Builder},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// The `Host` every helper sends: loopback, which the rebinding guard always accepts.
pub const HOST: &str = "127.0.0.1:8710";

/// A request builder for `method` and `uri`, carrying [`HOST`].
pub fn request_to(method: &str, uri: &str) -> Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, HOST)
}

/// Finishes `builder` with `body` as JSON.
fn with_json(builder: Builder, body: &serde_json::Value) -> Request<Body> {
    builder
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

/// Finishes `builder` with `body` as JSON, or with no body at all.
fn with_optional_json(builder: Builder, body: Option<&serde_json::Value>) -> Request<Body> {
    match body {
        Some(body) => with_json(builder, body),
        None => builder.body(Body::empty()).expect("request"),
    }
}

/// The whole body of `response`.
pub async fn body_bytes(response: axum::response::Response) -> axum::body::Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
}

/// Runs one request and hands back status, headers and the undecoded body.
pub async fn send_raw(
    router: &Router,
    request: Request<Body>,
) -> (StatusCode, HeaderMap, axum::body::Bytes) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    (status, headers, body_bytes(response).await)
}

/// Runs one request and decodes the JSON body; a body that is not JSON reads as `null`.
pub async fn send(router: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let (status, _, bytes) = send_raw(router, request).await;
    let payload = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, payload)
}

/// Like [`send`], but a body that is there and is not JSON fails the test.
///
/// An empty body still reads as `null`: a `204` has nothing to decode.
pub async fn send_strict(
    router: &Router,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let (status, _, bytes) = send_raw(router, request).await;
    let payload = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON body")
    };
    (status, payload)
}

/// Runs one request and returns the body as text, with the content type it was sent with.
pub async fn send_text(router: &Router, request: Request<Body>) -> (StatusCode, String, String) {
    let (status, headers, bytes) = send_raw(router, request).await;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// The session token a response's `Set-Cookie` carries, if it carries one.
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("rd_session="))
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Runs one request, decodes the JSON body and keeps the session a `Set-Cookie` hands out:
/// a sign-in is only proven by the session.
pub async fn send_with_cookie(
    router: &Router,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let (status, headers, bytes) = send_raw(router, request).await;
    let payload = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, payload, session_cookie(&headers))
}

/// Any method on a session route, with a JSON body or none.
pub async fn request(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    send(
        router,
        with_optional_json(request_to(method, uri), body.as_ref()),
    )
    .await
}

/// `GET` on a session route.
pub async fn get_json(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    request(router, "GET", uri, None).await
}

/// `POST` on a session route.
pub async fn post_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    request(router, "POST", uri, Some(body)).await
}

/// `POST` on a session route whose answer has to be JSON; see [`send_strict`].
pub async fn post_json_strict(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send_strict(router, with_json(request_to("POST", uri), &body)).await
}

/// `PUT` on a session route.
pub async fn put_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    request(router, "PUT", uri, Some(body)).await
}

/// `PATCH` on a session route.
pub async fn patch_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    request(router, "PATCH", uri, Some(body)).await
}

/// `DELETE` on a session route.
pub async fn delete_json(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    request(router, "DELETE", uri, None).await
}

/// A builder carrying `bearer` instead of a session.
fn bearer_request(method: &str, uri: &str, bearer: &str) -> Builder {
    request_to(method, uri).header(header::AUTHORIZATION, format!("Bearer {bearer}"))
}

/// `GET` with a bearer token instead of a session.
pub async fn get_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
) -> (StatusCode, serde_json::Value) {
    send(
        router,
        bearer_request("GET", uri, bearer)
            .body(Body::empty())
            .expect("request"),
    )
    .await
}

/// Any method with a bearer token and an empty JSON body.
///
/// Used by the scope matrix, which only cares whether the request is refused before it
/// reaches a handler; the body never has to be valid for the operation.
pub async fn request_with_bearer(
    router: &Router,
    method: &str,
    uri: &str,
    bearer: &str,
) -> (StatusCode, serde_json::Value) {
    send(
        router,
        with_json(bearer_request(method, uri, bearer), &serde_json::json!({})),
    )
    .await
}

/// `POST` with a bearer token and a JSON body.
pub async fn post_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(
        router,
        with_json(bearer_request("POST", uri, bearer), &body),
    )
    .await
}

/// `PATCH` with a bearer token and a JSON body.
pub async fn patch_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(
        router,
        with_json(bearer_request("PATCH", uri, bearer), &body),
    )
    .await
}

/// `PUT` with a bearer token and a JSON body.
pub async fn put_with_bearer(
    router: &Router,
    uri: &str,
    bearer: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(router, with_json(bearer_request("PUT", uri, bearer), &body)).await
}

/// `POST /api/v1/capture/batches` with the capture bearer.
pub async fn post_capture(
    router: &Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    post_with_bearer(
        router,
        "/api/v1/capture/batches",
        super::CAPTURE_BEARER,
        body,
    )
    .await
}

/// A builder carrying the session `token` as its cookie.
fn cookie_request(method: &str, uri: &str, token: &str) -> Builder {
    request_to(method, uri).header(header::COOKIE, format!("rd_session={token}"))
}

/// Signs a request with a session cookie rather than a bearer.
pub async fn get_with_cookie(
    router: &Router,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    send(
        router,
        cookie_request("GET", uri, token)
            .body(Body::empty())
            .expect("request"),
    )
    .await
}

pub async fn post_json_with_cookie(
    router: &Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(router, with_json(cookie_request("POST", uri, token), &body)).await
}

pub async fn put_json_with_cookie(
    router: &Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(router, with_json(cookie_request("PUT", uri, token), &body)).await
}

/// A POST whose `Set-Cookie` matters — the login, which is where a session token comes from.
pub async fn post_json_with_headers(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send_with_cookie(router, with_json(request_to("POST", uri), &body)).await
}

/// A `POST` on a session route whose own `Set-Cookie` matters.
///
/// The password change is the one route that both requires a session and hands back a new
/// one, because it ends every session including the caller's (RD-120-22). A test cannot see
/// that rotation through [`post_json_with_cookie`], which drops the response headers.
pub async fn post_json_with_cookie_and_headers(
    router: &Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send_with_cookie(router, with_json(cookie_request("POST", uri, token), &body)).await
}

/// Signs in and returns the raw `Set-Cookie` value, flags and all.
pub async fn login_cookie(router: &Router, password: &str) -> String {
    let (_, headers, _) = send_raw(
        router,
        with_json(
            request_to("POST", "/api/v1/auth/login"),
            &serde_json::json!({ "password": password }),
        ),
    )
    .await;
    headers
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Completes the setup with `password` unless it is complete already, signs in, and returns
/// the session token.
pub async fn sign_in(router: &Router, password: &str) -> String {
    let (status, body) = post_json(
        router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": password }),
    )
    .await;
    assert!(
        status.is_success() || body["code"] == "auth.setup_completed",
        "setup: {status} {body}"
    );
    let (status, body, token) = post_json_with_headers(
        router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": password }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login: {body}");
    token.expect("a session cookie")
}

/// One file, the way a browser uploads it: the multipart content type and the body.
///
/// `part_type` is the type the file part declares; the importers read the file name and
/// the bytes, so it only has to be plausible.
pub fn multipart(file_name: &str, part_type: &str, content: &[u8]) -> (String, Vec<u8>) {
    let boundary = "rdtestboundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {part_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Where a file import goes and what the uploaded file is called.
#[derive(Clone, Copy)]
pub struct Upload<'a> {
    pub uri: &'a str,
    pub file_name: &'a str,
    pub part_type: &'a str,
}

impl<'a> Upload<'a> {
    /// The same import, for a file called `file_name`.
    pub fn named<'b>(self, file_name: &'b str) -> Upload<'b>
    where
        'a: 'b,
    {
        Upload {
            uri: self.uri,
            file_name,
            part_type: self.part_type,
        }
    }
}

/// Uploads `content` as one multipart file to an import route.
pub async fn import(
    router: &Router,
    upload: Upload<'_>,
    content: &[u8],
) -> (StatusCode, serde_json::Value) {
    let (content_type, body) = multipart(upload.file_name, upload.part_type, content);
    send(
        router,
        request_to("POST", upload.uri)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}
