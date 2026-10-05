//! Pure template-expansion, credential-encoding and domain-gating helpers used by
//! [`super::host`].
//!
//! Split out of `host.rs` to keep every file under the workspace's 500-line limit; none of
//! this depends on `NativeHost` or touches the database, secret store or network directly.

use base64::{Engine, prelude::BASE64_STANDARD};
use rd_core::{Failure, FailureKind};
use rd_plugin_api::{HostHttpRequest, HostRequestValue};
use url::Url;

use super::references::{SECRET_MARKER_OPEN, Secrets, substitute};

mod gates;

use gates::header_forbids_credentials;
pub(super) use gates::{
    allowed_header, bearer_allowed, cookie_scope, http_failure, method_allowed,
    reference_active_for_account, secret_domain_allowed, transient, username_domain_allowed,
    validate_redirect, validate_request_domain,
};
pub use gates::{provider_cookie_scope, provider_download_bearer, provider_token_beside_the_flow};

pub(super) const USERNAME_MARKER: &str = "{{username}}";
/// `{{basic:<reference>}}`: the account's username and that secret, as one HTTP Basic blob
/// (RD-120-04).
///
/// It exists for a provider whose whole API is HTTP Basic — Seedr's REST v1 says in its own
/// documentation that it is "only available to use with HTTP basic auth" — and neither of the
/// two markers beside it can express that. `Authorization: Basic {{username}}:{{secret:…}}`
/// would send the pair unencoded, and a guest cannot encode them itself because it holds
/// neither: `{{secret:…}}` substitutes on the way *out*.
///
/// So the host does the one thing a guest cannot, and does no more than that. It is **both**
/// credentials at once, so it passes **both** gates: the reference has to be the account's
/// active slot and allowed for this address, exactly as a `{{secret:…}}` marker is, and the
/// username has to be allowed for it too. What comes back is base64 of `username:secret` and
/// nothing else — no header name, no scheme word — so a plugin still writes `Basic ` itself
/// and the marker cannot be turned into some other credential by the guest.
pub(super) const BASIC_MARKER_OPEN: &str = "{{basic:";
/// The OAuth client an installation registered for itself (RD-106-04).
///
/// Deliberately **not** a secret marker, and the difference is the point. A client id
/// identifies the application to the provider, not the person to the application: Google, and
/// every other provider of this shape, publishes it in the address the person is sent to. So it
/// is stored in the clear as the account's username, it is expanded into an authorization URL —
/// which no secret may ever be — and it does not make a request "carry credentials".
///
/// What it is instead is **per-installation configuration**, which is why it exists at all. A
/// client id compiled into this repository would sit in the git history, in every signed
/// `.rdplug` and in every release artifact, and every installation in the world would share one
/// project's quota with every other. Registered per installation, each one has its own.
pub const CLIENT_ID_MARKER: &str = "{{client_id}}";
/// The reference-less form. It means "the one secret this invocation was granted", which is
/// how the plugin types with no provider account behind them reach their credential: they
/// cannot name a reference, so they do not.
pub(super) const GRANTED_SECRET_MARKER: &str = "{{secret}}";
/// The same marker as it survives URL parsing. `Url` percent-encodes braces in a path, so a
/// plugin that writes the marker into an address gets this back — matching only the literal
/// form would find nothing and refuse a request that is perfectly well formed.
pub(super) const ENCODED_GRANTED_SECRET_MARKER: &str = "%7B%7Bsecret%7D%7D";

pub(super) fn account_missing() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        "plugin.account_missing",
        "Resolver account is missing",
    )
}

pub(super) fn secret_target_not_allowed() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "plugin.secret_target_not_allowed",
        "Secret must not be used for this resolver target",
    )
}

/// The first vault reference either credential marker names in `value`.
///
/// Only a presence test now: which references a request names, and what each one is filled
/// with, is [`super::references`]' business (RD-120-39).
fn credential_marker(value: &str) -> Option<&str> {
    secret_marker(value).or_else(|| basic_marker(value))
}

/// Whether a `{{client_id}}` marker appears anywhere in the request.
pub(super) fn has_client_id_marker(request: &HostHttpRequest) -> bool {
    request
        .query
        .iter()
        .chain(&request.headers)
        .any(|value| value.value_template.contains(CLIENT_ID_MARKER))
        || template_body(request).is_some_and(|body| body.contains(CLIENT_ID_MARKER))
}

/// Whether a `{{username}}` marker appears in the query, headers or a UTF-8 body.
///
/// A `{{basic:…}}` marker counts as one: half of what it expands to *is* the username, so the
/// host has to look it up and put it through the same domain gate before anything is built.
pub(super) fn has_username_marker(request: &HostHttpRequest) -> bool {
    let carries = |value: &str| value.contains(USERNAME_MARKER) || basic_marker(value).is_some();
    request
        .query
        .iter()
        .chain(&request.headers)
        .any(|value| carries(&value.value_template))
        || template_body(request).is_some_and(carries)
}

/// Whether a bare `{{username}}` marker appears, as opposed to only the half of one a
/// `{{basic:…}}` marker carries.
///
/// The empty user name a provider without `username_required` may use is an allowance for
/// the Basic pair alone (RD-120-38); a bare marker still refuses an empty name, as it did.
pub(super) fn has_bare_username_marker(request: &HostHttpRequest) -> bool {
    request
        .query
        .iter()
        .chain(&request.headers)
        .any(|value| value.value_template.contains(USERNAME_MARKER))
        || template_body(request).is_some_and(|body| body.contains(USERNAME_MARKER))
}

/// Whether the reference-less `{{secret}}` marker appears anywhere in the request —
/// including its address, which a webhook whose token is part of its path needs.
pub(super) fn has_granted_secret_marker(request: &HostHttpRequest) -> bool {
    url_carries_granted_secret(&request.url)
        || request
            .query
            .iter()
            .chain(&request.headers)
            .any(|value| value.value_template.contains(GRANTED_SECRET_MARKER))
        || template_body(request).is_some_and(|body| body.contains(GRANTED_SECRET_MARKER))
}

/// The vault reference a `{{basic:<reference>}}` marker names.
pub(super) fn basic_marker(value: &str) -> Option<&str> {
    let start = value.find(BASIC_MARKER_OPEN)? + BASIC_MARKER_OPEN.len();
    let end = value[start..].find("}}").map(|offset| start + offset)?;
    Some(&value[start..end])
}

pub(super) fn secret_marker(value: &str) -> Option<&str> {
    let start = value.find(SECRET_MARKER_OPEN)? + SECRET_MARKER_OPEN.len();
    let end = value[start..].find("}}").map(|offset| start + offset)?;
    Some(&value[start..end])
}

/// How a substituted credential has to be encoded for the document it lands in.
///
/// A credential is arbitrary user input, so substituting it verbatim into a structured body
/// is both a correctness and a security problem: a password containing `&` or `=` would not
/// merely arrive corrupted in a form post, it would split into extra form fields the site
/// then acts on (`x&op=logout`). Query and header values are encoded by the HTTP layer
/// itself and need nothing here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Escape {
    /// Substitute verbatim.
    None,
    /// Escape for a JSON string literal.
    Json,
    /// Percent-encode for an `application/x-www-form-urlencoded` body.
    Form,
}

impl Escape {
    fn apply(self, value: &str) -> String {
        match self {
            Self::None => value.to_owned(),
            Self::Json => json_escape(value),
            Self::Form => url::form_urlencoded::byte_serialize(value.as_bytes()).collect(),
        }
    }
}

fn secret_missing() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        "plugin.secret_missing",
        "Secret is missing",
    )
}

/// Expands the reference-less `{{secret}}` and every `{{secret:<reference>}}` marker, each
/// with its own value, encoding the substituted value as `escape` says.
///
/// A named marker is filled with the value loaded for **that** reference and nothing else
/// (RD-120-39). A reference nothing was loaded for refuses the request rather than borrowing a
/// neighbour's value -- which is what this function used to do, since it never compared the
/// name in the marker with the one that had been loaded.
pub(super) fn expand_template(
    value: &str,
    secrets: &Secrets,
    escape: Escape,
) -> Result<String, Failure> {
    let value = if value.contains(GRANTED_SECRET_MARKER) {
        let secret = secrets.granted().ok_or_else(secret_missing)?;
        value.replace(GRANTED_SECRET_MARKER, &escape.apply(secret))
    } else {
        value.to_owned()
    };
    substitute(&value, SECRET_MARKER_OPEN, |reference| {
        let secret = secrets.named(reference).ok_or_else(secret_missing)?;
        Ok(escape.apply(secret))
    })
}

/// Expands a `{{basic:<reference>}}` marker into base64 of `username:secret` (RD-120-04).
///
/// The one credential shape a guest could not otherwise send. The pair itself is built by
/// [`basic_credential`], which is also what the download engine's transfer path uses, so a
/// resolver's request and a transfer cannot disagree about what a Basic credential is.
///
/// The base64 output is still put through `escape`, because standard base64 spells `+`, `/`
/// and `=`, and all three mean something else inside a form body.
pub(super) fn expand_basic(
    value: &str,
    secrets: &Secrets,
    username: Option<&str>,
    username_optional: bool,
    escape: Escape,
) -> Result<String, Failure> {
    substitute(value, BASIC_MARKER_OPEN, |reference| {
        let secret = secrets.named(reference).ok_or_else(secret_missing)?;
        let encoded = basic_credential(secret, username, username_optional)?;
        Ok(escape.apply(&encoded))
    })
}

/// base64 of `username:secret`, the HTTP Basic credential, or the refusal that stands in for
/// half of one.
///
/// A missing user name refuses with the code the single markers use rather than substituting
/// an empty string, **unless the provider does not require one** (`username_optional`, which
/// is `!username_required` on its row, RD-120-38). The two cases really are different. For
/// Seedr the name is the account's e-mail address, and `Basic base64(":password")` is a request
/// that reaches the provider and comes back 401 -- which reads as an expired sign-in for an
/// account that was never complete. Pixeldrain asks for exactly that shape: its API key is the
/// Basic *password* and the user name is empty by its own documentation. The provider row is
/// what tells them apart, so the rule is the row's and not a guess made here.
///
/// A colon inside the username would be read by the provider as the end of it, so it is
/// refused here rather than encoded into something the provider will split differently. RFC
/// 7617 says the same: the user-id may not contain one.
pub(super) fn basic_credential(
    secret: &str,
    username: Option<&str>,
    username_optional: bool,
) -> Result<String, Failure> {
    let username = match username.filter(|value| !value.is_empty()) {
        Some(username) => username,
        None if username_optional => "",
        None => {
            return Err(Failure::coded(
                FailureKind::AuthRequired,
                "plugin.username_missing",
                "Account username is missing",
            ));
        }
    };
    if username.contains(':') {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "plugin.basic_username_invalid",
            "A HTTP Basic user name must not contain a colon",
        ));
    }
    Ok(BASE64_STANDARD.encode(format!("{username}:{secret}").as_bytes()))
}

/// Expands a `{{username}}` marker, encoding the substituted value as `escape` says.
pub(super) fn expand_username(
    value: &str,
    username: Option<&str>,
    escape: Escape,
) -> Result<String, Failure> {
    if !value.contains(USERNAME_MARKER) {
        return Ok(value.to_owned());
    }
    let username = username.ok_or_else(|| {
        Failure::coded(
            FailureKind::AuthRequired,
            "plugin.username_missing",
            "Account username is missing",
        )
    })?;
    let replacement = escape.apply(username);
    Ok(value.replace(USERNAME_MARKER, &replacement))
}

/// Escapes `"`, `\` and control characters so a value can be substituted into a JSON string.
pub(super) fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

pub(super) fn content_type_is_json(headers: &[HostRequestValue]) -> bool {
    content_type_contains(headers, "application/json")
}

pub(super) fn content_type_is_form(headers: &[HostRequestValue]) -> bool {
    content_type_contains(headers, "application/x-www-form-urlencoded")
}

fn content_type_contains(headers: &[HostRequestValue], needle: &str) -> bool {
    headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-type")
            && header.value_template.to_ascii_lowercase().contains(needle)
    })
}

/// The request body, if it is a template the host expands markers in.
///
/// Only a body the plugin declared as a form or as JSON is one: those are the two shapes a
/// sign-in is written in, and the two a substituted value can be encoded for. Anything else is
/// content — a file on its way to a storage destination, a `.torrent` handed to a remote job, a
/// multipart upload — and a file somebody else made that happens to read `{{secret}}` must
/// arrive as those bytes, not as the credential (RD-120-66). A `PUT` carries content by
/// definition, whatever it declares. Every question about markers in the body asks this, so a
/// body that is not expanded does not have credentials loaded for it either.
pub(super) fn template_body(request: &HostHttpRequest) -> Option<&str> {
    if body_escape(&request.headers) == Escape::None || request.method.eq_ignore_ascii_case("PUT") {
        return None;
    }
    std::str::from_utf8(&request.body).ok()
}

/// How a body's declared content type wants credentials encoded.
fn body_escape(headers: &[HostRequestValue]) -> Escape {
    if content_type_is_json(headers) {
        Escape::Json
    } else if content_type_is_form(headers) {
        Escape::Form
    } else {
        Escape::None
    }
}

/// Expands a `{{client_id}}` marker, encoding the substituted value as `escape` says.
///
/// Missing is a refusal with its own code rather than an empty substitution, because the
/// request that followed would be a sign-in attempt with no client at all — and the provider's
/// answer to that names nothing anybody can act on.
pub(super) fn expand_client_id(
    value: &str,
    client_id: Option<&str>,
    escape: Escape,
) -> Result<String, Failure> {
    if !value.contains(CLIENT_ID_MARKER) {
        return Ok(value.to_owned());
    }
    let client_id = client_id.ok_or_else(client_not_configured)?;
    let replacement = escape.apply(client_id);
    Ok(value.replace(CLIENT_ID_MARKER, &replacement))
}

/// The refusal for an OAuth provider whose installation has registered no client yet.
///
/// One code, one message, said the same way whether it is noticed while building the address
/// the person is sent to or while exchanging what came back — because it is one thing to do
/// about it either way.
#[must_use]
pub fn client_not_configured() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        "oauth.client_not_configured",
        "This provider needs an OAuth client of its own. Register one with the provider and \
         enter its client ID on the account.",
    )
}

/// Expands `{{secret:<reference>}}` and `{{username}}` markers in `request`'s query, headers
/// and (if it parses as UTF-8) body, in place. The substituted value is encoded for the body's
/// declared content type — JSON-string-escaped for `application/json`, percent-encoded for
/// `application/x-www-form-urlencoded`; query and header values are never escaped here, since
/// the HTTP layer encodes those itself.
///
/// Returns whether the request now carries credential material (a secret or username was
/// requested for it) — the caller uses that to decide whether a redirect must stay on the
/// manifest domain.
///
/// `username_optional` is the account's provider row saying it does not require a user name
/// (`username_required = false`); only then may a `{{basic:…}}` marker be built with an empty
/// one (RD-120-38). See [`basic_credential`].
pub(super) fn expand_request(
    request: &mut HostHttpRequest,
    secrets: &Secrets,
    username: Option<&str>,
    client_id: Option<&str>,
    username_optional: bool,
) -> Result<bool, Failure> {
    // A client id is not credential material — the provider publishes it in the address the
    // person is sent to — so it deliberately does not make a request carry credentials, and a
    // redirect is not narrowed on account of one.
    let carries_credential = !secrets.is_empty() || username.is_some();
    let escape = body_escape(&request.headers);
    for value in request.headers.iter() {
        if header_forbids_credentials(&value.name)
            && (credential_marker(&value.value_template).is_some()
                || value.value_template.contains(GRANTED_SECRET_MARKER)
                || value.value_template.contains(USERNAME_MARKER))
        {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.secret_target_not_allowed",
                "Secret must not be used for this resolver target",
            ));
        }
    }
    for value in request.query.iter_mut().chain(request.headers.iter_mut()) {
        // Before the single markers, so a `{{basic:…}}` reference is never first eaten by the
        // secret expansion it shares a name with.
        value.value_template = expand_basic(
            &value.value_template,
            secrets,
            username,
            username_optional,
            Escape::None,
        )?;
        value.value_template = expand_template(&value.value_template, secrets, Escape::None)?;
        value.value_template = expand_username(&value.value_template, username, Escape::None)?;
        value.value_template = expand_client_id(&value.value_template, client_id, Escape::None)?;
    }
    if let Some(body) = template_body(request)
        && (credential_marker(body).is_some()
            || body.contains(GRANTED_SECRET_MARKER)
            || body.contains(USERNAME_MARKER)
            || body.contains(CLIENT_ID_MARKER))
    {
        let expanded = expand_basic(body, secrets, username, username_optional, escape)?;
        let expanded = expand_template(&expanded, secrets, escape)?;
        let expanded = expand_username(&expanded, username, escape)?;
        let expanded = expand_client_id(&expanded, client_id, escape)?;
        request.body = expanded.into_bytes();
    }
    Ok(carries_credential)
}

/// Expands the granted secret inside a request address.
///
/// A webhook whose token *is* part of its address — Discord's is — cannot otherwise keep that
/// token in the vault. The expansion happens after the address has already passed the
/// manifest's domain gate, and the result is checked again: a secret may lengthen a path, it
/// may never move the request to another host or another scheme. An expansion that does not
/// parse is refused rather than half-applied.
pub(super) fn expand_url(url: &Url, secret: Option<&str>) -> Result<Url, Failure> {
    if !url_carries_granted_secret(url) {
        return Ok(url.clone());
    }
    let secret = secret.ok_or_else(|| {
        Failure::coded(
            FailureKind::AuthRequired,
            "plugin.secret_missing",
            "Secret is missing",
        )
    })?;
    let substituted = url
        .as_str()
        .replace(GRANTED_SECRET_MARKER, secret)
        .replace(ENCODED_GRANTED_SECRET_MARKER, secret);
    let expanded = Url::parse(&substituted).map_err(|_| secret_target_not_allowed())?;
    if expanded.scheme() != url.scheme() || expanded.host_str() != url.host_str() {
        return Err(secret_target_not_allowed());
    }
    Ok(expanded)
}

/// Whether an address carries the granted-secret marker, in either spelling.
fn url_carries_granted_secret(url: &Url) -> bool {
    let text = url.as_str();
    text.contains(GRANTED_SECRET_MARKER) || text.contains(ENCODED_GRANTED_SECRET_MARKER)
}

#[cfg(test)]
mod tests;
