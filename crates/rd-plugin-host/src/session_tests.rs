//! The shape a keyed session has to have, and what happens to everything else.

use super::{FlowValue, decode_key, parse, redactions};

const KEY: &str = "DExE4Sjq7npAvL1P_sGWFw";

#[test]
fn a_plain_token_stays_a_plain_token() {
    match parse("an-opaque-access-token").expect("a token") {
        FlowValue::Token(token) => assert_eq!(token, "an-opaque-access-token"),
        FlowValue::Keyed { .. } => panic!("a plain token was read as a session"),
    }
}

#[test]
fn a_keyed_session_splits_into_its_two_halves() {
    let value = format!(r#"{{"token":"session-id","key":"{KEY}"}}"#);
    match parse(&value).expect("a session") {
        FlowValue::Keyed { token, key } => {
            assert_eq!(token, "session-id");
            assert_eq!(key.as_str(), KEY);
        }
        FlowValue::Token(_) => panic!("a session was read as a token"),
    }
}

#[test]
fn an_object_of_another_shape_is_refused_rather_than_sent_as_a_token() {
    // Were this stored as a token, the next `{{secret:…}}` would send the whole object --
    // key included -- to the provider. That is the mistake the split exists to prevent.
    for value in [
        format!(r#"{{"sid":"session-id","mk":"{KEY}"}}"#),
        format!(r#"{{"token":"session-id","key":"{KEY}","extra":1}}"#),
        format!(r#"{{"token":"","key":"{KEY}"}}"#),
        r#"{"token":"session-id","key":"c2hvcnQ"}"#.to_owned(),
        r#"{"token":"session-id""#.to_owned(),
    ] {
        let failure = parse(&value).expect_err("must be refused");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.store_token_session_invalid")
        );
        assert!(
            !failure.message.contains("session-id") && !failure.message.contains(KEY),
            "the refusal quoted the value: {}",
            failure.message
        );
    }
}

#[test]
fn a_key_is_sixteen_bytes_exactly() {
    assert_eq!(decode_key(KEY).expect("sixteen bytes").len(), 16);
    assert_eq!(decode_key(&format!("{KEY}==")).expect("padding").len(), 16);
    assert!(decode_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err());
    assert!(decode_key("not base64 at all!").is_err());
}

#[test]
fn both_halves_are_redacted_on_their_own() {
    let value = format!(r#"{{"token":"session-id","key":"{KEY}"}}"#);
    let redacted = redactions(&value);
    assert!(redacted.contains(&value));
    assert!(redacted.contains(&"session-id".to_owned()));
    assert!(redacted.contains(&KEY.to_owned()));
}
