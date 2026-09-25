//! Building a failure the interface can translate, and keeping the operator's text out of it.

use plugin_common::{Failure, FailureKind};

use crate::brand::Brand;

/// Longest `error_name` that is carried as a parameter; anything longer is not a code.
const MAX_CODE_LEN: usize = 64;

/// A failure with one of the brand's codes and an English fallback naming the brand.
#[must_use]
pub fn coded(brand: &Brand, kind: FailureKind, code: &str, what: &str) -> Failure {
    Failure::coded(kind, code, format!("{}: {what}", brand.name))
}

/// Something the API sent that is meant to be a code — an `error_name` — reduced to what a code
/// may look like.
///
/// Nothing the operator wrote travels verbatim into a failure: a `message` may quote a file
/// name or a link, so it is never a parameter, and a value that is not shaped like a code
/// (letters, digits, underscore, at most 64 of them) loses all of itself rather than being
/// filtered down to its digits. The same rule `google-drive-common` applies.
#[must_use]
pub fn sanitize_code(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.is_empty()
        && trimmed.len() <= MAX_CODE_LEN
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

/// The failure for an answer this crate cannot read; `field` names what was missing or
/// malformed and never what it contained.
#[must_use]
pub fn invalid_response(brand: &Brand, field: &str) -> Failure {
    coded(
        brand,
        FailureKind::Permanent,
        brand.codes.invalid_response,
        &format!("the API answer could not be read ({field})"),
    )
    .with_param("field", field)
}
