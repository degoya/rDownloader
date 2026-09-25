//! The vault references one plugin request names, and the values the host loaded for them
//! (RD-120-39).
//!
//! A request may name more than one reference, and some have to: an OAuth renewal against a
//! provider whose person registered their own application sends the application's client
//! secret *and* the refresh material the sign-in stored, two credentials in two fields of one
//! request. Until RD-120-39 the host loaded only the first reference it found and wrote that
//! value into every marker, so such a renewal sent the client secret where the refresh token
//! belonged. Each reference is now loaded on its own, through the same gate a single one
//! passes, and each marker is filled with the value of the reference it names -- never with a
//! neighbour's.

use rd_core::{Failure, FailureKind};
use rd_plugin_api::HostHttpRequest;
use secrecy::{ExposeSecret, SecretString};

use super::expand::BASIC_MARKER_OPEN;

/// The `{{secret:<reference>}}` marker's opening.
pub(super) const SECRET_MARKER_OPEN: &str = "{{secret:";

/// How many distinct references one request may name.
///
/// Two is what a real request needs today (a client secret and a refresh token); four leaves
/// room without letting one request walk through the vault. Each reference still has to pass
/// its own gate, so the cap is not what keeps a foreign credential out -- it bounds how much
/// one call may load at all.
pub(super) const MAX_SECRET_REFERENCES: usize = 4;

/// The credentials one request resolved to: the invocation's granted secret, if the request
/// used the reference-less marker, and one value per named reference.
///
/// The values stay [`SecretString`]s until the moment they are substituted, so a `Debug` of
/// this type names references and never prints a value.
#[derive(Debug, Default)]
pub(super) struct Secrets {
    granted: Option<SecretString>,
    named: Vec<(String, SecretString)>,
}

impl Secrets {
    pub(super) fn set_granted(&mut self, value: SecretString) {
        self.granted = Some(value);
    }

    pub(super) fn insert(&mut self, reference: &str, value: SecretString) {
        self.named.push((reference.to_owned(), value));
    }

    /// The reference-less `{{secret}}` value.
    pub(super) fn granted(&self) -> Option<&str> {
        self.granted.as_ref().map(ExposeSecret::expose_secret)
    }

    /// The value loaded for exactly this reference, and for no other.
    pub(super) fn named(&self, reference: &str) -> Option<&str> {
        self.named
            .iter()
            .find(|(name, _)| name == reference)
            .map(|(_, value)| value.expose_secret())
    }

    /// Whether any credential was loaded at all.
    pub(super) fn is_empty(&self) -> bool {
        self.granted.is_none() && self.named.is_empty()
    }
}

/// Every reference a marker of the given opening names in `value`, in order.
///
/// A marker without its closing `}}` names nothing, which is how the single finder always
/// read it: it is left in place and never matches a reference.
pub(super) fn markers<'a>(value: &'a str, open: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let mut rest = value;
    std::iter::from_fn(move || {
        let start = rest.find(open)? + open.len();
        let end = rest[start..].find("}}")? + start;
        let reference = &rest[start..end];
        rest = &rest[end + 2..];
        Some(reference)
    })
}

/// Replaces every marker of the given opening in `value` with what `fill` answers for the
/// reference it names.
///
/// One pass over the original text, so a substituted value is never scanned again: a secret
/// that happens to contain `{{secret:other}}` arrives as that text rather than pulling a
/// second credential into the request.
pub(super) fn substitute(
    value: &str,
    open: &str,
    mut fill: impl FnMut(&str) -> Result<String, Failure>,
) -> Result<String, Failure> {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(found) = rest.find(open) {
        let start = found + open.len();
        let Some(end) = rest[start..].find("}}").map(|offset| start + offset) else {
            break;
        };
        output.push_str(&rest[..found]);
        output.push_str(&fill(&rest[start..end])?);
        rest = &rest[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

/// The distinct vault references the request's query, headers and UTF-8 body name, in the
/// order they first appear -- `{{secret:...}}` and `{{basic:...}}` alike, because a Basic marker
/// names a vault reference exactly as a secret marker does and passes the same gate.
///
/// More than [`MAX_SECRET_REFERENCES`] is refused here, before anything is loaded.
pub(super) fn secret_references(request: &HostHttpRequest) -> Result<Vec<&str>, Failure> {
    let body = super::expand::template_body(request);
    let texts = request
        .query
        .iter()
        .chain(&request.headers)
        .map(|value| value.value_template.as_str())
        .chain(body);
    let mut references: Vec<&str> = Vec::new();
    for text in texts {
        for reference in markers(text, SECRET_MARKER_OPEN).chain(markers(text, BASIC_MARKER_OPEN)) {
            if !references.contains(&reference) {
                references.push(reference);
            }
        }
    }
    if references.len() > MAX_SECRET_REFERENCES {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.secret_references_exceeded",
            format!("A plugin request may name at most {MAX_SECRET_REFERENCES} vault references"),
        )
        .with_param("limit", MAX_SECRET_REFERENCES));
    }
    Ok(references)
}

/// A request's secrets for a test that names at most one reference: `value` stands for that
/// reference and for the reference-less marker. A fixture naming two references must say
/// which value belongs to which, so it is refused here rather than silently given one value.
#[cfg(test)]
pub(super) fn single_for_tests(request: &HostHttpRequest, value: Option<&str>) -> Secrets {
    let references = secret_references(request).expect("within the cap");
    assert!(
        references.len() <= 1,
        "a fixture with two references must name each value"
    );
    let mut secrets = Secrets::default();
    if let Some(value) = value {
        secrets.set_granted(SecretString::from(value.to_owned()));
        for reference in references {
            secrets.insert(reference, SecretString::from(value.to_owned()));
        }
    }
    secrets
}

#[cfg(test)]
#[path = "references_tests.rs"]
mod tests;
