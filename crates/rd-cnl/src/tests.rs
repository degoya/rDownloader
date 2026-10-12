use std::time::Duration;

use aes::Aes128;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::{
    Encryptor,
    cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7},
};

use crate::jk::{JK_SLOTS, MAX_CONCURRENT_JK, extract_static_key, resolve_key, resolve_key_within};
use crate::{MAX_CRYPTED_BYTES, RefusalKind, decrypt, decrypt_with};

/// For tests that expect an evaluation to finish. The production budget of 250 ms was
/// overrun by Boa's first evaluation on a loaded GitHub runner (2026-09-25), which turned a
/// test of the result into a test of the machine; the expiry has its own tests.
async fn resolve_patiently(source: &str) -> anyhow::Result<[u8; 16]> {
    resolve_key_within(source, Duration::from_secs(10)).await
}

const KEY_HEX: &str = "abababababababababababababababab";

/// What a hoster page posts as `crypted`: the links, encrypted with the key as its own IV.
fn crypted(plaintext: &str) -> String {
    let key: &[u8; 16] = &[0xab; 16];
    let mut buffer = vec![0_u8; plaintext.len() + 16];
    buffer[..plaintext.len()].copy_from_slice(plaintext.as_bytes());
    let encrypted = Encryptor::<Aes128>::new(key.into(), key.into())
        .encrypt_padded::<Pkcs7>(&mut buffer, plaintext.len())
        .expect("encrypt")
        .to_vec();
    STANDARD.encode(encrypted)
}

#[test]
fn accepts_literal_or_javascript_wrapped_static_keys() {
    let expected = [0xab_u8; 16];
    assert_eq!(
        extract_static_key("function f(){ return 'abababababababababababababababab'; }")
            .expect("one unambiguous literal"),
        Some(expected)
    );
    // The `key` form: the field is the key itself.
    assert_eq!(
        extract_static_key("  abababababababababababababababab  ").expect("a bare key"),
        Some(expected)
    );
}

#[tokio::test]
async fn evaluates_non_literal_key_in_boa_without_host_apis() {
    let script =
        "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
    assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
}

#[tokio::test]
async fn rejects_unbounded_javascript_loops() {
    let script = "function getKey(){ while(true){} }";
    assert!(resolve_key(script).await.is_err());
}

/// The timeout used to end only the waiting. What it has to end now is the claim on the
/// runtime: the caller is still able to evaluate the next script right afterwards.
#[tokio::test]
async fn an_expired_jk_evaluation_leaves_the_caller_working() {
    let spinning = "function getKey(){ var n=0; while(true){ n=n+1; } }";
    let expired = resolve_key_within(spinning, Duration::from_millis(1)).await;
    assert!(expired.is_err(), "a script that never returns must not win");

    // The next caller is served, which is the property the blocking pool used to lose.
    let script =
        "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
    assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
}

/// A page must not be able to open one evaluation per request.
#[tokio::test]
async fn concurrent_jk_evaluations_are_capped() {
    let held = JK_SLOTS
        .try_acquire_many(u32::try_from(MAX_CONCURRENT_JK).expect("a small cap"))
        .expect("every slot is free at the start of this test");
    let script =
        "function getKey(){ return ['abababab','abababab','abababab','abababab'].join(''); }";
    let refused = resolve_key(script).await;
    assert!(
        refused.is_err(),
        "with every slot taken the call has to be refused, not queued"
    );
    drop(held);
    assert_eq!(resolve_patiently(script).await.ok(), Some([0xab_u8; 16]));
}

/// The old pattern matched anywhere, so an unrelated identifier or the first half of a
/// 64-digit literal silently became the key and decryption then failed with a padding
/// error that pointed at nothing.
#[tokio::test]
async fn an_ambiguous_static_key_is_refused_rather_than_guessed_at() {
    let two_literals = "function getKey(){ var id='0123456789abcdef0123456789abcdef'; \
                        return 'abababababababababababababababab'; }";
    assert!(
        extract_static_key(two_literals).is_err(),
        "two candidates cannot be resolved by taking the first"
    );
    assert!(resolve_key(two_literals).await.is_err());

    let sixty_four = format!("function getKey(){{ return '{}'; }}", "ab".repeat(32));
    assert_eq!(
        extract_static_key(&sixty_four).expect("no literal key, not an error"),
        None,
        "a 64-digit literal is not a 128-bit key and must not be cut in half"
    );
    // It is then evaluated, and the 64-digit result is refused with a reason.
    let error = resolve_patiently(&sixty_four)
        .await
        .expect_err("a 64-digit result is not a key");
    assert!(
        error.to_string().contains("128-bit hexadecimal key"),
        "{error}"
    );
}

#[tokio::test]
async fn a_payload_under_a_static_key_decrypts_to_its_links() {
    let links = "https://example.com/a.rar\r\nhttps://example.com/b.rar";
    let text = decrypt(&crypted(links), KEY_HEX)
        .await
        .expect("a well-formed payload");
    assert_eq!(text, links);
    let wrapped = format!("function f(){{ return '{KEY_HEX}'; }}");
    assert_eq!(
        decrypt(&crypted(links), &wrapped)
            .await
            .expect("a jk script"),
        links
    );
}

/// RD-1260-10: the service answers a refused `jk` with a code of its own, so the refusal has to
/// be told apart from a payload that does not decrypt -- and it has to win over one.
#[tokio::test]
async fn a_refused_jk_is_a_key_refusal_whatever_the_payload() {
    for jk in [
        "function getKey(){ while(true){} }",
        "no function and no key",
        "",
    ] {
        let refusal = decrypt("not base64 at all!", jk)
            .await
            .expect_err("a jk the sandbox refuses");
        assert_eq!(refusal.kind, RefusalKind::InvalidKey, "{jk:?}");
    }
}

#[test]
fn a_bad_payload_is_refused_as_one_and_says_nothing_else() {
    let key = [0xab_u8; 16];
    let refusal = decrypt_with("bm90IHBhZGRlZCBjb3JyZWN0bHkh", &key).expect_err("bad padding");
    assert_eq!(refusal.kind, RefusalKind::InvalidPayload);
    let refusal = decrypt_with("%%%", &key).expect_err("not base64");
    assert_eq!(refusal.kind, RefusalKind::InvalidPayload);
    let oversized = "A".repeat(MAX_CRYPTED_BYTES * 2 + 4);
    let refusal = decrypt_with(&oversized, &key).expect_err("too large");
    assert_eq!(refusal.kind, RefusalKind::TooLarge);
}
