use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use rd_core::DownloadId;

use super::{
    IDEMPOTENCY_KEY_HEADER, IdempotencyKey, IdempotencyStore, LIFETIME, MAX_KEY_LENGTH,
    MAX_PER_CALLER,
};
use crate::audit::Actor;

fn actor(id: &str) -> Actor {
    Actor {
        kind: rd_core::AuditActorKind::Token,
        id: Some(id.to_owned()),
        ..Actor::anonymous()
    }
}

fn key(caller: &str, value: &str) -> IdempotencyKey {
    IdempotencyKey::new(&actor(caller), value).expect("a valid key")
}

fn headers(values: &[&str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for value in values {
        headers.append(IDEMPOTENCY_KEY_HEADER, value.parse().expect("header value"));
    }
    headers
}

#[test]
fn a_request_without_the_header_has_no_key() {
    let none = IdempotencyKey::from_headers(&actor("a"), &HeaderMap::new()).expect("no header");
    assert_eq!(none, None);
}

#[test]
fn a_malformed_header_is_refused_with_its_code() {
    let long = "k".repeat(MAX_KEY_LENGTH + 1);
    for values in [
        vec![""],
        vec!["two words"],
        vec![long.as_str()],
        vec!["a", "b"],
    ] {
        let error = IdempotencyKey::from_headers(&actor("a"), &headers(&values))
            .expect_err("a malformed key");
        assert_eq!(error.code(), "request.idempotency_key_invalid");
    }
    let longest = "k".repeat(MAX_KEY_LENGTH);
    assert!(IdempotencyKey::new(&actor("a"), &longest).is_ok());
}

#[tokio::test]
async fn a_repeat_is_handed_what_the_first_request_created() {
    let store = IdempotencyStore::default();
    let first = store.claim(key("a", "k"), "body").expect("a new key");
    let created = DownloadId::new();
    *first.lock().await = Some(created);
    let repeat = store.claim(key("a", "k"), "body").expect("the same key");
    assert_eq!(*repeat.lock().await, Some(created));
}

#[tokio::test]
async fn another_body_under_a_used_or_running_key_is_refused() {
    let store = IdempotencyStore::default();
    let first = store.claim(key("a", "k"), "body").expect("a new key");
    let running = first.lock().await;
    let error = store
        .claim(key("a", "k"), "other")
        .expect_err("still running");
    assert_eq!(error.code(), "request.idempotency_key_reused");
    drop(running);
    *first.lock().await = Some(DownloadId::new());
    let error = store.claim(key("a", "k"), "other").expect_err("used");
    assert_eq!(error.code(), "request.idempotency_key_reused");
}

#[tokio::test]
async fn a_failed_request_frees_its_key_for_a_corrected_body() {
    let store = IdempotencyStore::default();
    let first = store.claim(key("a", "k"), "wrong").expect("a new key");
    drop(first.lock().await);
    let second = store.claim(key("a", "k"), "right").expect("free again");
    assert!(!Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn a_key_belongs_to_its_caller() {
    let store = IdempotencyStore::default();
    let first = store.claim(key("a", "k"), "body").expect("a new key");
    *first.lock().await = Some(DownloadId::new());
    let other = store.claim(key("b", "k"), "another").expect("a new key");
    assert_eq!(*other.lock().await, None);
}

#[test]
fn a_part_is_a_key_of_its_own() {
    let whole = key("a", "k");
    assert_ne!(whole.part(0), whole.part(1));
    assert_eq!(whole.part(1), key("a", "k").part(1));
}

#[tokio::test]
async fn a_key_is_forgotten_after_its_lifetime() {
    let store = IdempotencyStore::default();
    let start = Instant::now();
    let first = store
        .claim_at(key("a", "k"), "body", start)
        .expect("a new key");
    *first.lock().await = Some(DownloadId::new());
    let just_before = start + LIFETIME - Duration::from_secs(1);
    assert!(store.claim_at(key("a", "k"), "other", just_before).is_err());
    let later = store
        .claim_at(key("a", "k"), "other", start + LIFETIME)
        .expect("forgotten");
    assert_eq!(*later.lock().await, None);
}

#[test]
fn a_caller_loses_its_own_oldest_key_first() {
    let store = IdempotencyStore::default();
    let start = Instant::now();
    let kept = store
        .claim_at(key("b", "kept"), "body", start)
        .expect("a new key");
    let oldest = store
        .claim_at(key("a", "0"), "body", start)
        .expect("a new key");
    for number in 1..=MAX_PER_CALLER {
        let at = start + Duration::from_millis(number as u64);
        store
            .claim_at(key("a", &number.to_string()), "body", at)
            .expect("a new key");
    }
    let end = start + Duration::from_secs(1);
    let again = store
        .claim_at(key("b", "kept"), "body", end)
        .expect("remembered");
    assert!(Arc::ptr_eq(&kept, &again));
    let replaced = store
        .claim_at(key("a", "0"), "body", end)
        .expect("a new key");
    assert!(!Arc::ptr_eq(&oldest, &replaced));
}
