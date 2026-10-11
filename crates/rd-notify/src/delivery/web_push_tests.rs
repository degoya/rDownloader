use aws_lc_rs::agreement::{ECDH_P256, PrivateKey};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{
    PushOutcome, VapidKey, WebPushSubscription, are_push_keys, crypto, is_deliverable_push_address,
    is_push_address, judge, push_payload, send_push,
};
use crate::{
    delivery::Message,
    model::{NotificationEvent, Severity},
};

fn bytes(value: &str) -> Vec<u8> {
    crypto::decode(value).expect("base64url")
}

/// RFC 8291, appendix A: the example message, encrypted with the example's sender key and salt,
/// is the example's body byte for byte.
#[test]
fn encryption_matches_the_rfc_8291_example() {
    let plaintext = bytes("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
    assert_eq!(plaintext, b"When I grow up, I want to be a watermelon");
    let sender = PrivateKey::from_private_key(
        &ECDH_P256,
        &bytes("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw"),
    )
    .expect("sender key");
    let salt: [u8; 16] = bytes("DGv6ra1nlYgDCS1FRnbzlw")
        .try_into()
        .expect("16 bytes of salt");
    let ua_public = bytes(
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
    );
    let auth = bytes("BTBZMqHH6r4Tts7J_aSIgg");

    let body = crypto::encrypt_with(&sender, salt, &ua_public, &auth, &plaintext).expect("encrypt");

    assert_eq!(
        URL_SAFE_NO_PAD.encode(body),
        "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
    );
}

/// Two messages to the same browser never share a sender key or a salt.
#[test]
fn every_message_gets_its_own_key_and_salt() {
    let ua_public =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    let auth = "BTBZMqHH6r4Tts7J_aSIgg";
    let first = crypto::encrypt(ua_public, auth, b"same").expect("first");
    let second = crypto::encrypt(ua_public, auth, b"same").expect("second");
    // Salt (16), record size (4), key id length (1), sender key (65).
    assert_ne!(first[..16], second[..16]);
    assert_ne!(first[21..86], second[21..86]);
    assert_eq!(first[16..20], 4096_u32.to_be_bytes());
    assert_eq!(first[20], 65);
}

#[test]
fn keys_that_are_no_browser_keys_are_refused() {
    let point =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    assert!(are_push_keys(point, "BTBZMqHH6r4Tts7J_aSIgg"));
    // Padded base64 is the same key.
    assert!(are_push_keys(point, "BTBZMqHH6r4Tts7J_aSIgg=="));
    assert!(!are_push_keys(point, "BTBZMqHH6r4Tts7J"));
    assert!(!are_push_keys(
        "BTBZMqHH6r4Tts7J_aSIgg",
        "BTBZMqHH6r4Tts7J_aSIgg"
    ));
    assert!(!are_push_keys("not base64 !", "BTBZMqHH6r4Tts7J_aSIgg"));
    assert!(crypto::encrypt(point, "BTBZMqHH6r4Tts7J", b"x").is_err());
    assert!(crypto::encrypt(point, "BTBZMqHH6r4Tts7J_aSIgg", &[0; 4000]).is_err());
}

#[test]
fn a_push_address_is_https_with_a_host() {
    assert!(is_push_address("https://fcm.googleapis.com/fcm/send/abc"));
    assert!(is_push_address(" https://web.push.apple.com/QGuQyavXut "));
    assert!(!is_push_address(
        "http://updates.push.services.mozilla.com/wpush/v2/x"
    ));
    assert!(!is_push_address("mailto:someone@example.org"));
    assert!(!is_push_address("not an address"));
}

/// A resolver that knows a few names, and fails for every other one.
struct Names;

impl rd_http::HostLookup for Names {
    fn lookup<'a>(&'a self, host: &'a str) -> rd_http::LookupFuture<'a> {
        let answer = match host {
            "push.example.org" => Ok(vec!["93.184.215.14".parse().expect("address")]),
            "router.fritz.box" => Ok(vec!["192.168.178.1".parse().expect("address")]),
            "localhost" => Ok(vec!["127.0.0.1".parse().expect("address")]),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such name",
            )),
        };
        Box::pin(async move { answer })
    }
}

/// The subscription route and the sender share this rule: a loopback or private address, by
/// itself or behind a name, is refused when it is handed over (RD-1240-28); a public one, or a
/// name that does not resolve right now, is taken.
#[tokio::test]
async fn a_deliverable_push_address_is_public() {
    for refused in [
        "http://push.example.org/a",
        "https://127.0.0.1/push",
        "https://[::1]:8443/push",
        "https://192.168.178.20/push",
        "https://router.fritz.box/push",
        "https://localhost:8443/push",
    ] {
        assert!(
            !is_deliverable_push_address(refused, &Names).await,
            "{refused}"
        );
    }
    for taken in [
        "https://push.example.org/wpush/v2/abc",
        "https://203.0.113.9/push",
        "https://unknown.example.net/push",
    ] {
        assert!(is_deliverable_push_address(taken, &Names).await, "{taken}");
    }
}

/// The token is an ES256 JWT for the push service's origin, signed by the key it names, and the
/// key survives the vault's PKCS #8 form.
#[test]
fn the_vapid_header_carries_a_token_the_public_key_verifies() {
    let (key, pkcs8) = VapidKey::generate().expect("generate");
    let stored = VapidKey::from_pkcs8(&pkcs8).expect("read back");
    assert_eq!(stored.public_key(), key.public_key());
    let public_key = bytes(&key.public_key());
    assert_eq!(public_key.len(), 65);
    assert_eq!(public_key[0], 4);

    let endpoint = reqwest::Url::parse("https://push.example.org:8443/send/abc?x=1").expect("url");
    let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("time");
    let header = stored.authorization(&endpoint, now).expect("header");
    let (token, k) = header
        .strip_prefix("vapid t=")
        .and_then(|rest| rest.split_once(", k="))
        .expect("vapid t=..., k=...");
    assert_eq!(k, key.public_key());
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let head: serde_json::Value = serde_json::from_slice(&bytes(parts[0])).expect("header json");
    assert_eq!(head, serde_json::json!({ "typ": "JWT", "alg": "ES256" }));
    let claims: serde_json::Value = serde_json::from_slice(&bytes(parts[1])).expect("claims");
    assert_eq!(claims["aud"], "https://push.example.org:8443");
    assert_eq!(claims["exp"], 1_700_000_000 + 12 * 60 * 60);
    assert!(
        claims["sub"]
            .as_str()
            .is_some_and(|sub| sub.starts_with("https://"))
    );
    let signature = bytes(parts[2]);
    assert_eq!(signature.len(), 64);
    let signed = format!("{}.{}", parts[0], parts[1]);
    assert!(crypto::verifies(&public_key, signed.as_bytes(), &signature));
    assert!(!crypto::verifies(
        &public_key,
        b"something else",
        &signature
    ));
}

#[test]
fn an_unreadable_vapid_key_is_an_error() {
    assert!(VapidKey::from_pkcs8(b"not a key").is_err());
}

fn subscription(endpoint: &str, events: Vec<NotificationEvent>) -> WebPushSubscription {
    WebPushSubscription {
        id: "019d0000-0000-7000-8000-000000000001".to_owned(),
        endpoint: endpoint.to_owned(),
        device_name: "Firefox on Linux".to_owned(),
        events,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
            .to_owned(),
        auth: "BTBZMqHH6r4Tts7J_aSIgg".to_owned(),
    }
}

#[test]
fn a_subscription_without_events_wants_every_event() {
    let all = subscription("https://push.example.org/a", Vec::new());
    assert!(all.wants(NotificationEvent::PackageFailed));
    let some = subscription(
        "https://push.example.org/a",
        vec![NotificationEvent::PackageCompleted],
    );
    assert!(some.wants(NotificationEvent::PackageCompleted));
    assert!(!some.wants(NotificationEvent::PackageFailed));
    // The keys never reach an answer.
    let shown = serde_json::to_value(&some).expect("serialize");
    assert!(
        shown.get("p256dh").is_none() && shown.get("auth").is_none(),
        "{shown}"
    );
}

#[test]
fn the_payload_is_short_enough_to_encrypt_and_names_the_event() {
    let message = Message {
        title: "t".repeat(5_000),
        body: "b".repeat(5_000),
        event: NotificationEvent::BackupFailed,
        idempotency_key: "rule:event".to_owned(),
        payload: serde_json::json!({}),
    };
    let payload = push_payload(&message);
    let value: serde_json::Value = serde_json::from_slice(&payload).expect("json");
    assert_eq!(value["event"], "backup_failed");
    assert_eq!(value["tag"], "rule:event");
    assert_eq!(value["title"].as_str().map(str::len), Some(100));
    assert_eq!(value["body"].as_str().map(str::len), Some(500));
    let point =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    let body = crypto::encrypt(point, "BTBZMqHH6r4Tts7J_aSIgg", &payload).expect("fits");
    assert!(body.len() <= 4096);

    // The longest text there is: control characters, which JSON writes as six bytes each, and
    // characters of four bytes.
    let widest = Message {
        title: "\u{1f}".repeat(5_000),
        body: "\u{1f600}\u{1f}".repeat(5_000),
        event: NotificationEvent::PackageCompleted,
        idempotency_key: format!("{}:{}", "r".repeat(36), "e".repeat(36)),
        payload: serde_json::json!({}),
    };
    let payload = push_payload(&widest);
    let body = crypto::encrypt(point, "BTBZMqHH6r4Tts7J_aSIgg", &payload).expect("still fits");
    assert!(body.len() <= 4096, "{}", body.len());
}

/// A push address on this machine or in plain `http` is never posted to, and never retried.
#[tokio::test]
async fn a_push_address_inside_or_without_tls_is_refused() {
    let (key, _) = VapidKey::generate().expect("key");
    for endpoint in [
        "https://127.0.0.1:9/push/abc",
        "https://169.254.169.254/latest",
        "http://push.example.org/abc",
    ] {
        let outcome = send_push(
            &key,
            &subscription(endpoint, Vec::new()),
            b"{}",
            Severity::Info,
        )
        .await;
        assert!(
            matches!(
                outcome,
                PushOutcome::Failed {
                    retryable: false,
                    ..
                }
            ),
            "{endpoint}: {outcome:?}"
        );
    }
}

/// How `judge` reads a push service that answers with `status` and the body `nope`.
async fn judged(status: &str) -> PushOutcome {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    let answer = format!("HTTP/1.1 {status}\r\ncontent-length: 4\r\nconnection: close\r\n\r\nnope");
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await;
            let _ = stream.write_all(answer.as_bytes()).await;
        }
    });
    let response = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("answer");
    judge(response).await
}

/// 201 is delivered, 404 and 410 mean the browser dropped the subscription, the push service's
/// own trouble is retried and a refusal of the request is not.
#[tokio::test]
async fn the_push_service_answer_decides_the_outcome() {
    assert_eq!(judged("201 Created").await, PushOutcome::Delivered);
    assert_eq!(judged("404 Not Found").await, PushOutcome::Gone);
    assert_eq!(judged("410 Gone").await, PushOutcome::Gone);
    for (status, retried) in [
        ("429 Too Many Requests", true),
        ("503 Service Unavailable", true),
        ("403 Forbidden", false),
        ("413 Payload Too Large", false),
    ] {
        match judged(status).await {
            PushOutcome::Failed {
                retryable, detail, ..
            } => {
                assert_eq!(retryable, retried, "{status}");
                assert_eq!(detail, "nope", "{status}");
            }
            other => panic!("{status}: {other:?}"),
        }
    }
}
