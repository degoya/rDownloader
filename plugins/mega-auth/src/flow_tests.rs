//! What MEGA's two sign-in answers look like, and what this plugin reads out of them.

use super::{
    ACCOUNT_VERSION_2, api_error, b64_decode, b64_encode, preflight, preflight_body, session,
    session_id, sign_in_body, stored_session,
};

#[test]
fn the_two_request_bodies_carry_the_host_marker_and_never_an_address() {
    let body = String::from_utf8(preflight_body()).expect("utf-8");
    assert_eq!(body, r#"[{"a":"us0","user":"{{username}}"}]"#);
    let body = String::from_utf8(sign_in_body("AbCd")).expect("utf-8");
    assert_eq!(body, r#"[{"a":"us","user":"{{username}}","uh":"AbCd"}]"#);
}

#[test]
fn a_failure_arrives_as_a_negative_number_wrapped_or_bare() {
    // Measured behaviour: MEGA answers 200 for everything, and the failure is in the body
    // either inside the array or on its own.
    assert_eq!(api_error("[-9]"), Some(-9));
    assert_eq!(api_error("-26"), Some(-26));
    assert_eq!(api_error(" [ -3 ] "), Some(-3));
    assert_eq!(api_error(r#"[{"v":2,"s":"AAAA"}]"#), None);
}

#[test]
fn the_preflight_answer_yields_the_salt_and_the_version() {
    let answer = preflight(r#"[{"s":"bWVnYS1zYWx0","v":2}]"#).expect("readable");
    assert_eq!(answer.version, ACCOUNT_VERSION_2);
    assert_eq!(answer.salt, b"mega-salt");
    // A legacy account has no salt at all, and still parses, so the refusal can name the
    // version rather than saying the answer was unreadable.
    let legacy = preflight(r#"[{"v":1}]"#).expect("readable");
    assert_eq!(legacy.version, 1);
    assert!(legacy.salt.is_empty());
}

#[test]
fn the_sign_in_answer_is_refused_unless_every_block_has_a_usable_length() {
    let good = format!(
        r#"[{{"k":"{}","privk":"{}","csid":"{}"}}]"#,
        b64_encode(&[0x11; 16]),
        b64_encode(&[0x22; 32]),
        b64_encode(&[0x33; 256])
    );
    let answer = session(&good).expect("readable");
    assert_eq!(answer.wrapped_master_key.len(), 16);
    assert_eq!(answer.wrapped_private_key.len(), 32);
    assert_eq!(answer.encrypted_session_id.len(), 256);
    // A wrapped master key that is not one AES block is not one.
    let short = format!(
        r#"[{{"k":"{}","privk":"{}","csid":"{}"}}]"#,
        b64_encode(&[0x11; 8]),
        b64_encode(&[0x22; 32]),
        b64_encode(&[0x33; 256])
    );
    assert!(session(&short).is_none());
    assert!(session(r#"[{"k":"AAAA"}]"#).is_none());
}

#[test]
fn the_session_identifier_is_the_first_forty_three_bytes() {
    let plain = b"RDTESTSESSIONIDENTIFIER0123456789abcdefghijTAIL";
    let id = session_id(plain).expect("long enough");
    assert_eq!(id, b64_encode(&plain[..43]));
    assert!(session_id(b"too short").is_none());
}

#[test]
fn what_is_stored_holds_the_session_and_the_master_key_and_no_credential() {
    let stored = stored_session("SESSION", &[0xab; 16]);
    assert_eq!(
        stored,
        // The shape the host splits into a token it sends and a key it only computes with
        // (RD-120-30). Any other object is refused at `store-token`, never stored as a token.
        format!(
            r#"{{"token":"SESSION","key":"{}"}}"#,
            b64_encode(&[0xab; 16])
        )
    );
}

#[test]
fn megas_base64_survives_a_round_trip_and_the_standard_alphabet() {
    for length in 0..40_usize {
        let bytes: Vec<u8> = (0..length).map(|index| (index * 7 + 3) as u8).collect();
        let encoded = b64_encode(&bytes);
        assert_eq!(b64_decode(&encoded).expect("decodes"), bytes);
    }
    // `+` and `/` for `-` and `_`, and padding, are all accepted.
    assert_eq!(
        b64_decode("++//").expect("decodes"),
        b64_decode("--__").expect("decodes")
    );
    assert_eq!(b64_decode("bWVnYQ==").expect("decodes"), b"mega");
}
