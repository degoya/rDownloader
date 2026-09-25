//! What one answer about one key means, in every shape TorBox gives it.

use super::{Outcome, RATE_LIMIT_SECONDS, RETRY_SECONDS, read, retry_after_seconds};

fn body(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

#[test]
fn a_key_the_account_answers_for_is_signed_in() {
    assert_eq!(
        read(
            200,
            None,
            &body(
                r#"{"success":true,"detail":"ok","data":{"plan":2,"email":"a@example.invalid"}}"#
            )
        ),
        Outcome::Valid
    );
    // The free tier is still an account, and a person who signed in is signed in.
    assert_eq!(
        read(200, None, &body(r#"{"success":true,"data":{"plan":0}}"#)),
        Outcome::Valid
    );
}

/// The word decides before the status does, which is the whole reason this is not a status
/// check: TorBox answers a bad key with a `200`.
#[test]
fn a_refusal_inside_a_success_is_still_a_refusal() {
    assert_eq!(
        read(
            200,
            None,
            &body(r#"{"success":false,"error":"BAD_TOKEN","detail":"invalid api key abc123"}"#)
        ),
        Outcome::Invalid(crate::KEY_INVALID)
    );
    for word in ["AUTH_ERROR", "NO_AUTH", "OAUTH_VERIFICATION_ERROR"] {
        assert_eq!(
            read(200, None, &body(&format!(r#"{{"error":"{word}"}}"#))),
            Outcome::Invalid(crate::KEY_INVALID),
            "{word}"
        );
    }
    // And a status that says "not you", with no word to read.
    assert_eq!(read(401, None, b""), Outcome::Invalid(crate::KEY_INVALID));
    assert_eq!(read(403, None, b""), Outcome::Invalid(crate::KEY_INVALID));
}

/// "Not now" is a wait and never a refusal: a sign-in started during an outage has to finish
/// afterwards rather than tell somebody their key is wrong.
#[test]
fn an_answer_that_says_nothing_about_the_key_is_a_wait() {
    assert_eq!(read(503, None, b""), Outcome::Retry(RETRY_SECONDS));
    assert_eq!(read(429, None, b""), Outcome::Retry(RATE_LIMIT_SECONDS));
    // TorBox's own figure wins where it stated one.
    assert_eq!(read(429, Some(90), b""), Outcome::Retry(90));
    assert_eq!(
        read(200, Some(45), &body(r#"{"error":"TOO_MANY_REQUESTS"}"#)),
        Outcome::Retry(45)
    );
    assert_eq!(
        read(200, None, &body(r#"{"error":"DATABASE_ERROR"}"#)),
        Outcome::Retry(RETRY_SECONDS)
    );
    // A proxy's error page is not evidence about somebody's key.
    assert_eq!(
        read(200, None, b"<html>gateway timeout</html>"),
        Outcome::Retry(RETRY_SECONDS)
    );
    // A success with no account in it is not an answer either.
    assert_eq!(
        read(200, None, &body(r#"{"success":true,"data":null}"#)),
        Outcome::Retry(RETRY_SECONDS)
    );
}

#[test]
fn a_date_shaped_retry_after_is_ignored_rather_than_guessed_at() {
    assert_eq!(retry_after_seconds(Some(" 120 ")), Some(120));
    assert_eq!(
        retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        None
    );
    assert_eq!(retry_after_seconds(None), None);
}
