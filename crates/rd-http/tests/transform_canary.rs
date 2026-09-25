//! The key canaries (RD-110-33), after the pattern of RD-110-02.
//!
//! "Decryption keys never appear in logs or UI URLs" is an acceptance criterion, and an
//! acceptance criterion that rests on everybody remembering not to print something is not one.
//! So the key is put where a careless caller would reach for it -- a `Debug` line, a serialised
//! description, an error message, a tracing field, the central redaction -- and none of it may
//! come back out.
//!
//! The canary is the key itself, in every spelling it could plausibly be rendered in: raw hex,
//! upper case, the base64 the vault stores, and the decimal a `Debug` of a byte slice produces.

use rd_core::{
    CIPHER_AES_128_CTR, CipherSpec, ContentTransform, INTEGRITY_CBC_MAC_CHAIN, IntegritySpec,
    TransformKey,
};
use rd_http::{StreamTransform, TransformCheckpoint};

/// The key, and the reference it is reached by once it is in the vault.
const KEY: [u8; 16] = [
    0x0c, 0x4c, 0x44, 0xe1, 0x28, 0xea, 0xee, 0x7a, 0x40, 0xbc, 0xbd, 0x4f, 0xfe, 0xc1, 0x96, 0x17,
];
const REFERENCE: &str = "vault://019d0000-0000-7000-8000-0000000000aa";

/// Every spelling the key could survive as.
fn canaries() -> Vec<String> {
    let hex: String = KEY.iter().map(|byte| format!("{byte:02x}")).collect();
    let decimal: String = KEY
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        hex.clone(),
        hex.to_uppercase(),
        decimal,
        // The base64 the vault writes.
        "DExE4Sjq7npAvL1P/sGWFw==".to_owned(),
    ]
}

fn assert_clean(label: &str, rendered: &str) {
    for canary in canaries() {
        assert!(
            !rendered.contains(&canary),
            "{label}: the key survived as {canary:?} in: {rendered}"
        );
    }
}

fn description() -> ContentTransform {
    ContentTransform {
        cipher: CipherSpec {
            algorithm: CIPHER_AES_128_CTR.to_owned(),
            key_reference: Some(REFERENCE.to_owned()),
            nonce: vec![0x80, 0x1b, 0x72, 0xfd, 0x96, 0x41, 0xcc, 0xfa],
            first_block: 0,
        },
        integrity: Some(IntegritySpec {
            algorithm: INTEGRITY_CBC_MAC_CHAIN.to_owned(),
            boundaries: vec![300],
            iv: [0x80, 0x1b, 0x72, 0xfd, 0x96, 0x41, 0xcc, 0xfa].repeat(2),
            expected: vec![0; 8],
        }),
    }
}

/// The description is what gets written down, broadcast and shown. It carries a reference.
#[test]
fn the_written_description_carries_no_key() {
    let json = serde_json::to_string(&description()).expect("json");
    assert_clean("serialised description", &json);
    assert!(json.contains(REFERENCE), "{json}");
    assert_clean("printed description", &format!("{:?}", description()));
}

/// The type that does hold the bytes prints a placeholder.
#[test]
fn the_key_type_prints_a_placeholder() {
    let key = TransformKey::new(KEY.to_vec());
    let printed = format!("{key:?}");
    assert_clean("TransformKey debug", &printed);
    assert!(printed.contains("[redacted]"), "{printed}");
}

/// Everything the engine's transform can say about itself, and every refusal it can raise.
#[test]
fn no_refusal_and_no_state_of_the_transform_carries_the_key() {
    let key = TransformKey::new(KEY.to_vec());
    let transform = StreamTransform::new(description(), &key).expect("computable");
    assert_clean("fingerprint", transform.fingerprint());
    assert_clean("description", &format!("{:?}", transform.description()));

    // The integrity refusal, which is the one refusal computed *from* the key.
    let mut walker = transform.mac_walker(0).expect("an integrity value");
    let plaintext: Vec<u8> = (0..300).map(|index| (index % 251) as u8).collect();
    let mut macs = std::collections::BTreeMap::new();
    for (index, mac) in walker.feed(0, &plaintext).expect("in order") {
        macs.insert(index, mac);
    }
    let failure = transform
        .verify(&macs)
        .expect_err("the expectation is zeros");
    assert_clean("integrity refusal", &failure.message);
    assert_clean("redacted refusal", &rd_core::redact_text(&failure.message));

    // The out-of-order refusal, raised while the key is in scope.
    let mut walker = transform.mac_walker(0).expect("an integrity value");
    let out_of_order = walker.feed(17, &plaintext).expect_err("out of order");
    assert_clean("ordering refusal", &out_of_order.message);

    // And the refusal for a transform whose key was never put away. Matched rather than
    // unwrapped because `StreamTransform` has no `Debug` at all -- which is itself part of
    // what this file is about.
    let mut unstored = description();
    unstored.cipher.key_reference = None;
    match StreamTransform::new(unstored, &key) {
        Err(missing) => {
            assert_eq!(missing.code.as_deref(), Some(rd_core::CODE_KEY_MISSING));
            assert_clean("key-missing refusal", &missing.message);
        }
        Ok(_) => panic!("a transform without a vault reference was accepted"),
    }
}

/// The checkpoint is persisted and read back; it carries a fingerprint, not a key.
#[test]
fn the_checkpoint_carries_a_fingerprint_and_not_a_key() {
    let transform =
        StreamTransform::new(description(), &TransformKey::new(KEY.to_vec())).expect("computable");
    let checkpoint = TransformCheckpoint {
        fingerprint: Some(transform.fingerprint().to_owned()),
        macs: vec![(0, [9_u8; 16])],
    };
    assert_clean("checkpoint", &format!("{checkpoint:?}"));
}

/// The central redaction replaces a vault reference wherever one is printed, so even the
/// reference does not travel into a support log as something to look up.
#[test]
fn the_central_redaction_replaces_the_vault_reference() {
    let line = format!("resolved stream key {REFERENCE} for this download");
    let redacted = rd_core::redact_text(&line);
    assert!(!redacted.contains("019d0000"), "{redacted}");
    assert!(redacted.contains("vault://[redacted]"), "{redacted}");
}
