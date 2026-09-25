//! Box's protocol logic, written once for both builds.
//!
//! Everything here goes through the official Box Content API and nothing else:
//! `/2.0/files/<id>` for what a file is, `/2.0/users/me` for the account. The bytes come from
//! `/2.0/files/<id>/content`, which the resolver does not call itself: it answers with that
//! address and the scheduler fetches from there, with the account's token attached because
//! `api.box.com` is the provider's `secret_domains`. Box answers that request with a redirect
//! to a pre-authenticated address on its own storage host, which the transfer follows and which
//! carries its own authorization — so the bearer is dropped on the way out of `api.box.com`,
//! and never travels to a host Box did not sign the address for.
//!
//! **The download address is the stable one, and it is pinned to a version.** A resolver that
//! answered with the `dl.boxcloud.com` address would hand back something whose only identity is
//! an expiring signature, and a restart would have nothing left to ask for (RD-106-04, rule 5).
//! The API route can be asked again — `rd_scheduler::replay::before_resume` does exactly that —
//! and `?version=<file_version.id>` is what makes the answer mean one set of bytes rather than
//! "whatever is in that file now". Without the pin, a transfer that outlived an edit would
//! splice the head of one file onto the tail of another and still look complete; with it, Box
//! serves the version the partial file came from or refuses, and the SHA-1 handed back beside
//! it is the one of that same version.
//!
//! The account's access token is never in this file. Requests carry the marker
//! `{{secret:box_access_token}}`, which the host expands on the way out and only towards
//! `api.box.com`; the sibling OAuth plugin is what puts a value behind it.
//!
//! A password-protected shared link is opened the one way the API offers, `shared_link_password`
//! inside the `boxapi` header. The password comes from the address — `?shared_link_password=` or
//! `?password=` on the pasted link, both redacted by the core — because a resolver sees nothing
//! else of what a person entered, and it leaves this plugin in that header and nowhere else.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};

use box_common::{address, reason};

use crate::{
    api, messages,
    target::{self, Target},
};

/// The vault reference the Box provider keeps its access token under. The value never reaches
/// this plugin.
const SECRET: &str = "box_access_token";
/// The metadata one resolve needs, and not one field more. `fields` is not decoration at Box:
/// naming it replaces the standard set rather than adding to it, so an unnamed field is one
/// that does not travel.
const ITEM_FIELDS: &str = "type,name,size,sha1,file_version,item_status";
/// The name the checksum verifier knows a plain SHA-1 under.
const SHA1: &str = "sha1";
/// Most links one `check` call looks up. A check is one request per link, so an unbounded batch
/// is an unbounded number of requests inside one invocation's budget.
const MAX_CHECKS: usize = 50;

/// Whether this plugin claims `url`. Answered from the address alone and reaching nothing.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    target::claim(url).is_some()
}

/// The hosts this plugin serves, for the account's catalogue.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(vec![address::WEB_HOST.to_owned()])
}

/// What the account is, from `/2.0/users/me`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_token(host, account_id).await?;
    let response = call(
        host,
        HttpRequest::get(format!("{}/users/me", address::API)).with_query("fields", "login,name"),
        false,
    )
    .await?;
    let user = api::user(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    Ok(Account {
        valid: true,
        // A Box account is a Box account: no plan changes what this plugin may do, so claiming
        // a tier would be decoration.
        premium: false,
        label: Label::new()
            .user(account_name(user.login.as_deref(), user.name.as_deref()))
            .into(),
        // Deliberately not `space_amount` minus `space_used`. That is space left to *upload*
        // into, and reporting it as remaining traffic would tell somebody with a full Box that
        // they cannot download from it — which is not true.
        traffic_left: None,
    })
}

/// Turns one Box address into one download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let claimed = target::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_BOX_LINK, FailureKind::Unsupported))?;
    let item = fetch_item(host, &claimed).await?;
    if item.is_gone() {
        return Err(refuse(messages::FILE_NOT_FOUND, FailureKind::Permanent));
    }
    if item.is_folder() {
        // The sibling crawler's address, pasted at the resolver. Said plainly rather than as
        // "this file has no bytes", which is what a folder's metadata looks like.
        return Err(refuse(messages::IS_A_FOLDER, FailureKind::Unsupported));
    }
    if !item.is_file() {
        // A bookmark or anything else Box keeps in a folder that is not a file: there are no
        // bytes, and offering it would produce a queue entry that can only fail.
        return Err(refuse(messages::NOT_A_FILE, FailureKind::Unsupported));
    }
    Ok(Resolved {
        // The stable API route, pinned to the version Box just described, rather than the
        // pre-authenticated `dl.boxcloud.com` address it redirects to.
        url: address::content_address(claimed.id(), item.version()),
        file_name: item.name.clone().filter(|name| !name.is_empty()),
        size: item.size.as_ref().and_then(api::Flexible::as_u64),
        // The shared link the file is reached through, with its password. A file in the
        // account's own Box carries no header at all — the bearer the scheduler attaches is
        // the whole of its authorization.
        headers: claimed
            .box_api()
            .map(|value| Header::new("boxapi", value))
            .into_iter()
            .collect(),
        // Box's SHA-1 is over the whole file, so it is a plain digest and is named as one. It
        // belongs to the version the address above is pinned to, which is what makes it worth
        // checking at the end of a resumed transfer.
        checksum: item.sha1().map(|value| (SHA1.to_owned(), value)),
    })
}

/// Whether each of a batch of links is still there.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    input: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let mut results = Vec::new();
    for url in input.urls.iter().take(MAX_CHECKS) {
        let Some(claimed) = target::claim(url) else {
            results.push(unknown(url));
            continue;
        };
        results.push(match fetch_item(host, &claimed).await {
            Ok(item) if item.is_gone() => offline(url),
            Ok(item) if item.is_file() => LinkCheck {
                url: url.clone(),
                status: LinkStatus::Online,
                file_name: item.name.clone().filter(|name| !name.is_empty()),
                size: item.size.as_ref().and_then(api::Flexible::as_u64),
            },
            // A folder says nothing about a file link: it stays unknown.
            Ok(_) => unknown(url),
            // A file Box says is gone is offline; anything else says nothing about the link, so
            // it stays unknown rather than being reported as missing.
            Err(failure) if failure.code.as_deref() == Some(messages::FILE_NOT_FOUND.0) => {
                offline(url)
            }
            Err(_) => unknown(url),
        });
    }
    Ok(results)
}

/// One metadata call, with the shared link the item is reached through when there is one.
async fn fetch_item<H: PluginHost>(host: &H, target: &Target) -> Result<api::Item, Failure> {
    let mut request = HttpRequest::get(format!("{}/files/{}", address::API, target.id()))
        .with_query("fields", ITEM_FIELDS);
    if let Some(value) = target.box_api() {
        request = request.with_header("boxapi", value);
    }
    let response = call(host, request, target.is_shared()).await?;
    api::item(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// Makes one request and turns every answer that is not one into a refusal.
///
/// The whole vocabulary of "Box said no" lives here, so no caller decides a second time what a
/// status code meant — which is how a password and a permission end up under one message.
async fn call<H: PluginHost>(
    host: &H,
    request: HttpRequest,
    shared: bool,
) -> Result<HttpResponse, Failure> {
    let response = host
        .http(
            request
                .with_header("Authorization", format!("Bearer {{{{secret:{SECRET}}}}}"))
                .with_header("Accept", "application/json"),
        )
        .await?;
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    let code = reason::of(&response.body);
    let retry = response
        .header("retry-after")
        .and_then(|value| value.trim().parse::<u64>().ok());
    // Box repeats the status inside the document; the header is what is read, because a
    // document is a thing the answer carried and the status is the answer itself.
    let ((stable, message), kind) = api::classify(response.status, code.as_deref(), retry, shared);
    let mut failure = Failure::coded(kind, stable, message);
    if let Some(code) = code {
        // Sanitised in `reason::of` before it ever gets here, so an error document that quoted
        // a token publishes nothing.
        failure = failure.with_param("reason", code);
    }
    Err(failure)
}

/// Refuses early when the account holds no token at all, rather than making a call that Box is
/// certain to refuse and reporting whatever it says about it.
async fn require_token<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if host.secret_available(account_id, SECRET).await {
        return Ok(());
    }
    Err(refuse(
        messages::SIGN_IN_REQUIRED,
        FailureKind::AuthRequired,
    ))
}

fn account(account_id: Option<&str>) -> Result<&str, Failure> {
    account_id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| refuse(messages::ACCOUNT_MISSING, FailureKind::AuthRequired))
}

fn refuse((code, message): (&str, &str), kind: FailureKind) -> Failure {
    Failure::coded(kind, code, message)
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}

fn offline(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Offline,
        file_name: None,
        size: None,
    }
}

/// The account row's label: the address the person signed in with, and nothing else; the
/// display name only when the address is missing, and nothing at all when both are. The shared
/// label builder bounds it and strips controls, because it is shown rather than parsed.
fn account_name<'a>(login: Option<&'a str>, display_name: Option<&'a str>) -> Option<&'a str> {
    login
        .filter(|value| !value.is_empty())
        .or(display_name.filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::{Label, account_name, matches};

    #[test]
    fn only_box_file_addresses_are_claimed() {
        assert!(matches("https://app.box.com/file/123456789"));
        assert!(matches("https://app.box.com/s/abc123/file/42"));
        assert!(matches("https://contoso.app.box.com/file/1"));
        // A folder is the crawler's, so is a shared link that has not said what it is, and a
        // stranger's host is nobody's.
        assert!(!matches("https://app.box.com/folder/987"));
        assert!(!matches("https://app.box.com/s/abc123"));
        assert!(!matches("https://ddownload.com/f/abc"));
    }

    #[test]
    fn the_account_label_is_the_address_and_never_a_control_character() {
        assert_eq!(
            account_name(Some("someone@example.invalid"), Some("Someone")),
            Some("someone@example.invalid")
        );
        assert_eq!(account_name(None, Some("Someone")), Some("Someone"));
        assert_eq!(account_name(None, None), None);
        // Bounded and stripped of controls on its way into the label part.
        let parts = Label::new()
            .user(account_name(Some("a\u{0}b"), None))
            .user(account_name(Some(&"x".repeat(200)), None))
            .into_parts();
        assert_eq!(parts[0].params, vec![("user".to_owned(), "ab".to_owned())]);
        assert_eq!(parts[1].params[0].1.len(), 80);
        assert!(
            Label::new()
                .user(account_name(None, None))
                .into_parts()
                .is_empty()
        );
    }
}
