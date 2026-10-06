//! Small checks every area used to write out for itself (audit 1.9.1, API-11): a required text
//! field, a name's length, the header of an exported bundle, and an answer body read with a cap.
//!
//! Each one existed two to thirty times with its own spelling, so a limit or a code could drift
//! between routes without anyone noticing. The codes and messages stay the caller's; only the
//! mechanics live here.

use crate::ApiError;

/// The stored digest of a credential or the content hash of an upload, for the areas that do not
/// depend on `rd-authn` themselves.
pub use rd_authn::sha256_hex;

/// What a text field's upper bound counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextLimit {
    /// No bound beyond the request body's.
    Unbounded,
    /// At most this many characters -- names and labels a person types.
    Chars(usize),
    /// At most this many bytes -- identifiers, addresses and tokens a machine compares.
    Bytes(usize),
}

/// `value` trimmed, or `400 code` when nothing but whitespace is left or the trimmed text passes
/// `limit`.
///
/// A bounded refusal carries the bound as the `max` parameter, so the interface can say it.
///
/// # Errors
///
/// `400 code` with `message` (and `max` for a bounded field).
pub fn required_text(
    value: &str,
    limit: TextLimit,
    code: &'static str,
    message: impl Into<String>,
) -> Result<String, ApiError> {
    let trimmed = value.trim();
    let (too_long, max) = match limit {
        TextLimit::Unbounded => (false, None),
        TextLimit::Chars(max) => (trimmed.chars().count() > max, Some(max)),
        TextLimit::Bytes(max) => (trimmed.len() > max, Some(max)),
    };
    if trimmed.is_empty() || too_long {
        let error = ApiError::bad_request(code, message);
        return Err(match max {
            Some(max) => error.with_param("max", max),
            None => error,
        });
    }
    Ok(trimmed.to_owned())
}

/// `value` trimmed, or `None` when it is absent or nothing but whitespace is left: an optional
/// field a person cleared is no value, not an empty one.
#[must_use]
pub fn optional_text<S: AsRef<str>>(value: Option<S>) -> Option<String> {
    value
        .map(|value| value.as_ref().trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// `400 code` unless `value`, trimmed, is between 1 and `max` characters long.
///
/// # Errors
///
/// `400 code` with the parameters `min` (1) and `max`.
pub fn name_length(value: &str, code: &'static str, max: usize) -> Result<(), ApiError> {
    let length = value.trim().chars().count();
    if !(1..=max).contains(&length) {
        return Err(ApiError::bad_request(
            code,
            format!("Name must be between 1 and {max} characters long"),
        )
        .with_param("min", 1)
        .with_param("max", max));
    }
    Ok(())
}

/// What one kind of exported bundle says about itself, and how a file that is not one, or one of
/// a version this build cannot read, is refused.
#[derive(Clone, Copy, Debug)]
pub struct BundleHeader {
    /// The `format` the file must name.
    pub format: &'static str,
    /// The newest version this build writes and reads.
    pub version: u32,
    /// Whether an older version is still read; otherwise only `version` is.
    pub reads_older: bool,
    /// Code and message for a file of another format.
    pub format_code: &'static str,
    pub format_message: &'static str,
    /// Code and message for a version this build cannot read; the version goes into `params`.
    pub version_code: &'static str,
    pub version_message: &'static str,
}

impl BundleHeader {
    /// `400` unless `format` and `version` describe a bundle of this kind this build reads.
    ///
    /// # Errors
    ///
    /// `400 format_code`, or `400 version_code` with the file's version as the `version`
    /// parameter.
    pub fn check(&self, format: &str, version: u32) -> Result<(), ApiError> {
        if format != self.format {
            return Err(ApiError::bad_request(self.format_code, self.format_message));
        }
        let readable = if self.reads_older {
            version <= self.version
        } else {
            version == self.version
        };
        if !readable {
            return Err(
                ApiError::bad_request(self.version_code, self.version_message)
                    .with_param("version", version),
            );
        }
        Ok(())
    }
}

/// `415 request.content_type_unsupported` unless the request declares `expected` as its media
/// type (parameters such as `charset` aside).
///
/// For the routes that read their body as raw bytes (audit 1.9.1, API-01): a page of another
/// site can send `text/plain`, a form or `multipart/form-data` without asking the browser
/// first, but never `application/octet-stream` or `application/json` -- so demanding the
/// declared type puts a preflight in front of every cross-site attempt, which this service
/// answers without CORS. The origin check in `auth` is the first line; this is the second.
///
/// # Errors
///
/// `415 request.content_type_unsupported` with the `expected` parameter.
pub fn require_media_type(
    headers: &axum::http::HeaderMap,
    expected: &'static str,
) -> Result<(), ApiError> {
    let declared = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if declared.is_some_and(|declared| declared.eq_ignore_ascii_case(expected)) {
        return Ok(());
    }
    Err(ApiError::unsupported_media_type(
        "request.content_type_unsupported",
        format!("This route reads a body of type {expected}"),
    )
    .with_param("expected", expected))
}

/// Why an answer body could not be read whole.
#[derive(Debug)]
pub enum BodyError {
    /// The declared or the received length passed the cap.
    TooLarge,
    /// The connection failed while the body arrived.
    Interrupted(reqwest::Error),
}

/// Reads an answer body of at most `max_bytes`, refusing a longer one.
///
/// The peer is configurable, so the body is bounded as it arrives rather than trusted to be
/// short: a declared length is checked first, and a chunked answer that keeps coming is
/// abandoned before it passes the cap.
///
/// # Errors
///
/// [`BodyError::TooLarge`] past the cap, [`BodyError::Interrupted`] for a broken transfer.
pub async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, BodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(BodyError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Interrupted)? {
        if body.len() + chunk.len() > max_bytes {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads at most the first `max_bytes` of an answer body and drops the rest -- for a reader that
/// wants a bounded prefix rather than a refusal.
///
/// # Errors
///
/// The transport's error for a broken transfer.
pub async fn read_body_prefix(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() >= max_bytes {
            body.truncate(max_bytes);
            break;
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_text_trims_and_refuses_blank_and_long_values() {
        assert_eq!(
            required_text("  name ", TextLimit::Chars(4), "x.code", "msg").ok(),
            Some("name".to_owned())
        );
        let blank = required_text(" \t", TextLimit::Unbounded, "x.blank", "msg");
        assert_eq!(
            blank.err().map(|error| error.code().to_owned()),
            Some("x.blank".to_owned())
        );
        // Four characters, eight bytes: the limit counts what it says it counts.
        assert!(
            required_text(
                "\u{e4}\u{e4}\u{e4}\u{e4}",
                TextLimit::Chars(4),
                "x.code",
                "msg"
            )
            .is_ok()
        );
        let long = required_text(
            "\u{e4}\u{e4}\u{e4}\u{e4}",
            TextLimit::Bytes(4),
            "x.long",
            "msg",
        );
        let message = long.err().map(ApiError::into_message);
        assert_eq!(
            message.as_ref().map(|message| message.code.as_str()),
            Some("x.long")
        );
        assert_eq!(
            message.and_then(|message| message.params.get("max").cloned()),
            Some("4".to_owned())
        );
    }

    #[test]
    fn name_length_counts_trimmed_characters_against_the_given_limit() {
        assert!(name_length(" ab ", "x.name", 2).is_ok());
        assert!(name_length("   ", "x.name", 2).is_err());
        let message = name_length("abc", "x.name", 2)
            .err()
            .map(ApiError::into_message);
        assert_eq!(
            message.as_ref().map(|message| message.code.as_str()),
            Some("x.name")
        );
        assert_eq!(
            message.and_then(|message| message.params.get("max").cloned()),
            Some("2".to_owned())
        );
    }

    const HEADER: BundleHeader = BundleHeader {
        format: "test-bundle",
        version: 2,
        reads_older: false,
        format_code: "test.format",
        format_message: "not a test bundle",
        version_code: "test.version",
        version_message: "unsupported test bundle version",
    };

    #[test]
    fn a_bundle_header_names_the_refused_version_in_params() {
        assert!(HEADER.check("test-bundle", 2).is_ok());
        assert_eq!(
            HEADER
                .check("other", 2)
                .err()
                .map(|error| error.code().to_owned()),
            Some("test.format".to_owned())
        );
        let message = HEADER
            .check("test-bundle", 1)
            .err()
            .map(ApiError::into_message);
        assert_eq!(
            message.as_ref().map(|message| message.code.as_str()),
            Some("test.version")
        );
        assert_eq!(
            message.as_ref().map(|message| message.message.as_str()),
            Some("unsupported test bundle version")
        );
        assert_eq!(
            message.and_then(|message| message.params.get("version").cloned()),
            Some("1".to_owned())
        );
        let older = BundleHeader {
            reads_older: true,
            ..HEADER
        };
        assert!(older.check("test-bundle", 1).is_ok());
        assert!(older.check("test-bundle", 3).is_err());
    }

    #[test]
    fn a_raw_body_route_takes_only_its_declared_media_type() {
        let declaring = |value: &'static str| {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static(value),
            );
            headers
        };
        let json = "application/json";
        assert!(require_media_type(&declaring("application/json"), json).is_ok());
        assert!(require_media_type(&declaring("Application/JSON; charset=utf-8"), json).is_ok());
        for simple in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
        ] {
            assert_eq!(
                require_media_type(&declaring(simple), json)
                    .err()
                    .map(|error| error.code().to_owned()),
                Some("request.content_type_unsupported".to_owned()),
                "{simple}"
            );
        }
        assert!(require_media_type(&axum::http::HeaderMap::new(), json).is_err());
    }

    fn answer(body: &'static [u8]) -> reqwest::Response {
        reqwest::Response::from(axum::http::Response::new(body.to_vec()))
    }

    #[tokio::test]
    async fn a_body_within_the_cap_is_read_and_a_longer_one_refused() {
        assert_eq!(
            read_bounded_body(answer(b"12345"), 5).await.ok(),
            Some(b"12345".to_vec())
        );
        assert!(matches!(
            read_bounded_body(answer(b"123456"), 5).await,
            Err(BodyError::TooLarge)
        ));
    }

    #[tokio::test]
    async fn a_prefix_read_keeps_the_first_bytes() {
        assert_eq!(
            read_body_prefix(answer(b"123456"), 4).await.ok(),
            Some(b"1234".to_vec())
        );
        assert_eq!(
            read_body_prefix(answer(b"12"), 4).await.ok(),
            Some(b"12".to_vec())
        );
    }
}
