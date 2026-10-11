use super::{
    REDACTION_PLACEHOLDER, Redacted, error_with_causes, is_signed_url, redact_header_value,
    redact_text, redact_url, signed_url_expiry,
};
use url::Url;

#[derive(Debug)]
struct Layer {
    text: &'static str,
    cause: Option<Box<Layer>>,
}

impl std::fmt::Display for Layer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.text)
    }
}

impl std::error::Error for Layer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

#[test]
fn an_error_names_the_cause_its_sources_carry() {
    let error = Layer {
        text: "error sending request",
        cause: Some(Box::new(Layer {
            text: "client error (Connect)",
            cause: Some(Box::new(Layer {
                text: "dns error: failed to lookup address information",
                cause: None,
            })),
        })),
    };
    assert_eq!(
        error_with_causes(&error),
        "error sending request: client error (Connect): dns error: failed to lookup address \
             information"
    );
}

#[test]
fn a_cause_already_in_the_message_is_not_repeated_and_secrets_stay_redacted() {
    let error = Layer {
        text: "request to https://cdn.example/f?X-Amz-Signature=abc failed: timed out",
        cause: Some(Box::new(Layer {
            text: "timed out",
            cause: None,
        })),
    };
    let text = error_with_causes(&error);
    assert!(!text.contains("abc"), "{text}");
    assert_eq!(text.matches("timed out").count(), 1, "{text}");
}

fn url(input: &str) -> Url {
    input.parse().expect("url")
}

#[test]
fn plain_urls_are_returned_unchanged() {
    // Byte-for-byte, so ordinary logs do not churn just because redaction exists.
    let plain = "https://example.com/dir/file.bin?page=2&sort=name";
    assert_eq!(redact_url(&url(plain)), plain);
}

/// Redaction deliberately leaves a fragment standing (RD-109-32 confirmed RD-108-07).
///
/// A fragment is an anchor far more often than a secret, and this function is not only a
/// log helper: `replay_handlers` puts its result into a REST answer and `collector_enqueue`
/// into `downloads.source_path`, where a shortened address would simply be the wrong
/// address. The one address that could carry a password -- a pasted share link -- loses its
/// fragment before it is ever stored (`rd_core::candidate_url`), so nothing reaching here
/// from a candidate has one left to hide.
#[test]
fn a_fragment_is_left_alone_because_it_is_an_anchor_far_more_often_than_a_secret() {
    let anchor = "https://example.com/manual#installation";
    assert_eq!(redact_url(&url(anchor)), anchor);
    assert_eq!(
        crate::candidate_url(&url(anchor)).as_str(),
        "https://example.com/manual",
        "the storage rule is where a fragment is dropped, not redaction"
    );
}

#[test]
fn signature_values_are_replaced_and_names_kept() {
    let redacted = redact_url(&url(
        "https://cdn.example.com/f.bin?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Signature=deadbeefcafe&X-Amz-Expires=900",
    ));
    assert!(redacted.contains("X-Amz-Signature="));
    assert!(!redacted.contains("deadbeefcafe"));
    // The lifetime is not a secret and is needed to decide staleness.
    assert!(redacted.contains("X-Amz-Expires=900"));
}

#[test]
fn userinfo_is_stripped_from_proxy_urls() {
    let redacted = redact_url(&url("http://agent:hunter2@proxy.internal:8080/"));
    assert!(!redacted.contains("hunter2"));
    assert!(redacted.contains("proxy.internal"));
}

#[test]
fn remote_transfer_urls_lose_their_password_in_free_text() {
    // An FTP link is normally pasted *with* its credentials, so this is the common
    // case rather than an exotic one.
    for (input, secret) in [
        (
            "connecting to ftp://bob:hunter2@files.example.com/pub/a.bin",
            "hunter2",
        ),
        (
            "sftp://root:s3cr3t@10.0.0.5/srv/backup.tar failed",
            "s3cr3t",
        ),
        ("ftps://u:p%40ss@files.example.com/x refused", "p%40ss"),
        // A proxy profile handed to yt-dlp or gallery-dl (RD-1240-08).
        (
            "Unable to connect to proxy socks5h://alice:pr0xy@proxy.example.com:1080",
            "pr0xy",
        ),
        ("socks5://alice:pr0xy@10.0.0.5:1080 refused", "pr0xy"),
    ] {
        let redacted = redact_text(input);
        assert!(!redacted.contains(secret), "{input} -> {redacted}");
        assert!(redacted.contains("example.com") || redacted.contains("10.0.0.5"));
    }
}

#[test]
fn redaction_is_idempotent() {
    // The same value passes engine -> scheduler -> database -> SSE.
    for input in [
        "https://cdn.example.com/f?X-Amz-Signature=abc&X-Amz-Expires=900",
        "http://user:pw@proxy.internal:8080/",
        "Authorization: Bearer sk-livetoken",
        "Cookie: session=abc; other=def",
        "fetched vault://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b for the job",
        "error sending request for url (https://cdn.example.com/f?sig=zzz)",
        "nothing sensitive here at all",
    ] {
        let once = redact_text(input);
        assert_eq!(redact_text(&once), once, "not idempotent: {input}");
    }
}

#[test]
fn credential_header_lines_lose_their_value() {
    assert_eq!(
        redact_text("Authorization: Bearer sk-livetoken"),
        format!("Authorization: {REDACTION_PLACEHOLDER}")
    );
    assert_eq!(
        redact_text("Cookie: session=abc; other=def"),
        format!("Cookie: {REDACTION_PLACEHOLDER}")
    );
    assert_eq!(
        redact_header_value("Set-Cookie", "session=abc"),
        REDACTION_PLACEHOLDER
    );
    // A non-credential header keeps its value.
    assert_eq!(redact_header_value("Accept", "text/html"), "text/html");
}

#[test]
fn bare_auth_schemes_are_redacted_mid_sentence() {
    let redacted = redact_text("sent Bearer sk-livetoken to the origin");
    assert!(!redacted.contains("sk-livetoken"));
    assert!(redacted.contains("Bearer [redacted]"));
    // A word merely ending in the scheme name must not trigger.
    assert_eq!(redact_text("membasic value"), "membasic value");
}

#[test]
fn vault_references_never_survive() {
    let redacted = redact_text("stored as vault://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.");
    assert!(!redacted.contains("0190a1b2"));
    assert!(redacted.ends_with('.'), "punctuation kept: {redacted}");
}

#[test]
fn urls_embedded_in_text_and_json_are_found() {
    let redacted = redact_text(
        "error sending request for url (https://cdn.example.com/f?sig=zzz&e=1)\n\
             {\"source\":\"https://cdn.example.com/g?token=secretvalue\"}",
    );
    assert!(!redacted.contains("zzz"));
    assert!(!redacted.contains("secretvalue"));
    // Structure around the URL survives.
    assert!(redacted.contains("{\"source\":\""));
}

#[test]
fn signed_urls_are_recognised() {
    assert!(is_signed_url(&url("https://c.example/f?X-Amz-Signature=a")));
    assert!(is_signed_url(&url(
        "https://c.example/f?Expires=1735689600"
    )));
    assert!(!is_signed_url(&url("https://c.example/f?page=2")));
}

#[test]
fn expiry_is_read_from_each_signing_convention() {
    let aws = signed_url_expiry(&url(
        "https://c.example/f?X-Amz-Date=20240101T000000Z&X-Amz-Expires=900",
    ))
    .expect("aws expiry");
    assert_eq!(aws.to_rfc3339(), "2024-01-01T00:15:00+00:00");

    let azure = signed_url_expiry(&url("https://c.example/f?se=2024-01-01T00%3A15%3A00Z"))
        .expect("azure expiry");
    assert_eq!(azure.to_rfc3339(), "2024-01-01T00:15:00+00:00");

    let cloudfront =
        signed_url_expiry(&url("https://c.example/f?Expires=1704070800")).expect("cf expiry");
    assert_eq!(cloudfront.timestamp(), 1_704_070_800);

    // A millisecond timestamp is not a second-precision epoch and must not be believed.
    assert!(signed_url_expiry(&url("https://c.example/f?Expires=1704070800000")).is_none());
    assert!(signed_url_expiry(&url("https://c.example/f?page=2")).is_none());
}

#[test]
fn indexer_and_tracker_keys_are_secret_parameters() {
    for name in [
        "apikey", "api_key", "API-Key", "token", "key", "passkey", "authkey", "rsstoken",
    ] {
        let address = url(&format!(
            "https://indexer.example/api?t=get&{name}=K3Y&id=7"
        ));
        let redacted = redact_url(&address);
        assert!(!redacted.contains("K3Y"), "{name}: {redacted}");
        assert!(redacted.contains("id=7"), "{name}: {redacted}");
    }
}

#[test]
fn a_url_in_free_text_with_nothing_to_hide_keeps_its_exact_text() {
    let text = "see https://Example.COM and https://example.com/a?page=2.";
    assert_eq!(redact_text(text), text);
}

#[test]
fn display_wrapper_matches_the_function() {
    let signed = url("https://c.example/f?X-Amz-Signature=abc");
    assert_eq!(Redacted(&signed).to_string(), redact_url(&signed));
}
