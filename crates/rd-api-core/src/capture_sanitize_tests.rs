use super::*;

fn link() -> Url {
    "https://hoster.example/dl/1".parse().expect("url")
}

/// Sanitizes against a fixed link URL; most cases do not care which one it is.
fn sanitize_link(request: CapturedRequest) -> Result<SanitizedRequest, ApiError> {
    sanitize(&link(), request)
}

fn request(headers: Vec<(&str, &str)>) -> CapturedRequest {
    CapturedRequest {
        method: "get".to_owned(),
        headers: headers
            .into_iter()
            .map(|(name, value)| CapturedHeader {
                name: name.to_owned(),
                value: value.to_owned(),
            })
            .collect(),
        ..CapturedRequest::default()
    }
}

/// A POST carrying `body` with the given content type.
fn post(content_type: &str, body: &[u8]) -> CapturedRequest {
    CapturedRequest {
        method: "POST".to_owned(),
        headers: vec![CapturedHeader {
            name: "content-type".to_owned(),
            value: content_type.to_owned(),
        }],
        body_b64: Some(STANDARD.encode(body)),
        ..CapturedRequest::default()
    }
}

#[test]
fn keeps_allowlisted_headers_and_normalises_the_method() {
    let (sanitized, _) = sanitize_link(request(vec![("Accept", "*/*"), ("ACCEPT-LANGUAGE", "de")]))
        .expect("sanitized");
    assert_eq!(sanitized.method, "GET");
    assert_eq!(
        sanitized
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>(),
        ["accept", "accept-language"]
    );
}

#[test]
fn drops_credential_and_unknown_headers() {
    let (sanitized, _) = sanitize_link(request(vec![
        ("Cookie", "sid=1"),
        ("Authorization", "Bearer x"),
        ("X-Api-Key", "secret"),
        ("Proxy-Authorization", "Basic x"),
        ("Accept-Encoding", "gzip"),
        ("Range", "bytes=0-"),
        ("Accept", "*/*"),
    ]))
    .expect("sanitized");
    assert_eq!(sanitized.headers.len(), 1);
    assert_eq!(sanitized.headers[0].name, "accept");
}

#[test]
fn rejects_methods_other_than_get_and_post() {
    for method in ["PUT", "DELETE", "PATCH", "TRACE"] {
        let mut payload = request(Vec::new());
        payload.method = method.to_owned();
        assert_eq!(
            sanitize_link(payload).expect_err("rejected").code(),
            "capture.method_unsupported",
            "{method}"
        );
    }
    // POST is what contract v2 added.
    let mut payload = request(Vec::new());
    payload.method = "POST".to_owned();
    let (sanitized, body) = sanitize_link(payload).expect("post accepted");
    assert_eq!(sanitized.method, "POST");
    // An empty-body POST is legitimate and reproducible.
    assert!(body.is_none());
    assert!(sanitized.replayable);
}

#[test]
fn rejects_too_many_headers() {
    let headers = vec![("accept", "*/*"); MAX_CAPTURED_HEADERS + 1];
    assert_eq!(
        sanitize_link(request(headers))
            .expect_err("rejected")
            .code(),
        "capture.headers_limit"
    );
}

#[test]
fn rejects_oversized_values() {
    let long = "a".repeat(MAX_CAPTURED_VALUE + 1);
    assert_eq!(
        sanitize_link(request(vec![("accept", long.as_str())]))
            .expect_err("rejected")
            .code(),
        "capture.header_length"
    );
    let mut payload = request(Vec::new());
    payload.referrer = Some(long);
    assert_eq!(
        sanitize_link(payload).expect_err("rejected").code(),
        "capture.field_length"
    );
}

#[test]
fn drops_empty_fields_and_non_web_effective_urls() {
    let mut payload = request(Vec::new());
    payload.referrer = Some("   ".to_owned());
    payload.effective_url = Some("file:///etc/passwd".parse().expect("url"));
    let (sanitized, _) = sanitize_link(payload).expect("sanitized");
    assert!(sanitized.referrer.is_none());
    assert!(sanitized.effective_url.is_none());
}

#[test]
fn a_form_post_keeps_its_field_names_but_never_its_values() {
    let (sanitized, bytes) = sanitize_link(post(
        "application/x-www-form-urlencoded",
        b"id=42&token=s3cr3t&name=movie.mkv",
    ))
    .expect("sanitized");
    let body = sanitized.body.clone().expect("body metadata");
    assert_eq!(body.field_names, ["id", "token", "name"]);
    assert!(body.stored);
    assert_eq!(body.byte_len, 33);
    // The plaintext goes to the caller for vaulting and is cleared from the request.
    assert_eq!(
        bytes.as_deref(),
        Some(&b"id=42&token=s3cr3t&name=movie.mkv"[..])
    );
    assert!(sanitized.body_b64.is_none());
    let serialized = serde_json::to_string(&sanitized).expect("serialize");
    assert!(!serialized.contains("s3cr3t"), "{serialized}");
}

#[test]
fn unreproducible_bodies_are_accepted_but_blocked_with_a_reason() {
    // Refusing these with a 400 would make the link vanish: the extension has already
    // cancelled the browser download by the time the server answers.
    let mut upload = post("application/x-www-form-urlencoded", b"id=1");
    upload.has_file_upload = true;
    let (sanitized, bytes) = sanitize_link(upload).expect("accepted");
    assert!(!sanitized.replayable);
    assert_eq!(
        sanitized.blocked_reason,
        Some(ReplayBlockReason::FileUpload)
    );
    assert!(bytes.is_none(), "a refused body must not be stored");
    assert!(!sanitized.body.expect("metadata kept").stored);

    let (multipart, _) =
        sanitize_link(post("multipart/form-data; boundary=x", b"--x--")).expect("accepted");
    assert_eq!(
        multipart.blocked_reason,
        Some(ReplayBlockReason::MultipartUnsupported)
    );

    let (odd, _) = sanitize_link(post("application/octet-stream", b"\x00\x01")).expect("ok");
    assert_eq!(
        odd.blocked_reason,
        Some(ReplayBlockReason::ContentTypeUnsupported)
    );

    let big = vec![b'a'; MAX_REPLAY_BODY_BYTES + 1];
    let (oversize, bytes) = sanitize_link(post("application/json", &big)).expect("accepted");
    assert_eq!(
        oversize.blocked_reason,
        Some(ReplayBlockReason::BodyTooLarge)
    );
    assert!(bytes.is_none());
}

#[test]
fn structurally_broken_bodies_are_rejected() {
    let mut get_with_body = request(Vec::new());
    get_with_body.body_b64 = Some(STANDARD.encode(b"id=1"));
    assert_eq!(
        sanitize_link(get_with_body).expect_err("rejected").code(),
        "capture.body_not_allowed"
    );

    let mut not_base64 = post("application/json", b"{}");
    not_base64.body_b64 = Some("!!!not base64!!!".to_owned());
    assert_eq!(
        sanitize_link(not_base64).expect_err("rejected").code(),
        "capture.body_invalid"
    );

    let mut too_long = post("application/json", b"{}");
    too_long.body_b64 = Some("A".repeat(MAX_REPLAY_BODY_B64 + 1));
    assert_eq!(
        sanitize_link(too_long).expect_err("rejected").code(),
        "capture.body_length"
    );
}

#[test]
fn origins_and_expiry_are_server_derived_and_client_values_ignored() {
    let mut payload = request(Vec::new());
    // A client claiming a wider permission than it earned must not get it.
    payload.approved_origins = vec!["https://evil.example".to_owned()];
    payload.replayable = true;
    payload.effective_url = Some(
        "https://cdn.example.net/f.bin?X-Amz-Date=20240101T000000Z&X-Amz-Expires=900"
            .parse()
            .expect("url"),
    );
    let (sanitized, _) = sanitize_link(payload).expect("sanitized");
    assert_eq!(
        sanitized.approved_origins,
        ["https://hoster.example", "https://cdn.example.net"]
    );
    assert!(
        !sanitized
            .approved_origins
            .contains(&"https://evil.example".to_owned())
    );
    // The deadline is in 2024, so this capture is already expired.
    assert!(sanitized.expires_at.is_some());
    assert!(!sanitized.replayable);
    assert_eq!(sanitized.blocked_reason, Some(ReplayBlockReason::Expired));
}

/// RD-109-39: the refusal of an unparsable address carries no part of it, anywhere.
///
/// The prose is the half that matters here: it is what a request log writes, and it is what
/// the interface shows for any code no catalogue translates. It is a constant sentence over
/// a number, so it cannot hold an address -- and the function is never handed one, which is
/// the shape this test pins. The whole answer, parameters included, is checked end to end by
/// `an_address_that_does_not_parse_comes_back_in_no_answer_and_no_log_line`.
#[test]
fn a_refused_address_is_named_by_its_position_and_nothing_else() {
    let error = link_url_invalid(2);
    assert_eq!(error.code(), "collector.link_url_invalid");
    assert_eq!(error.message(), "Link 2 in the batch is not a valid URL");
}

#[test]
fn code_convention_is_followed() {
    let pattern = regex::Regex::new(r"^[a-z]+\.[a-z0-9_]+$").expect("regex");
    for error in [
        links_limit(100),
        method_unsupported(),
        headers_limit(32),
        header_length(),
        field_length("referrer"),
        link_url_invalid(1),
        body_not_allowed(),
        body_length(MAX_REPLAY_BODY_B64),
        body_invalid(),
    ] {
        assert!(pattern.is_match(error.code()), "{}", error.code());
    }
}
