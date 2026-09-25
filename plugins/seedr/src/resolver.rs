//! Seedr's protocol logic, written once for both builds.
//!
//! Everything here goes through the official REST API v1 and nothing else: `GET /rest/user` for
//! the account, and no other call at all. There is no scraping of the web interface and no
//! guessing at storage hostnames — those are the tricks that break, and the ones the job's
//! cross-cutting requirement rules out.
//!
//! The account's password is never in this file, and neither is its e-mail address. Requests
//! carry `seedr_common::address::AUTHORIZATION_TEMPLATE`, the host pairs the two, encodes them
//! and sends the result towards `www.seedr.cc` and nowhere else.
//!
//! **Two of the five exports deliberately reach nothing**, and that is a fact about Seedr
//! rather than a shortcut. Its documented Files section has no metadata call: `GET
//! /rest/file/{id}` *is* the download, and the rest of the section renames, deletes or renders
//! a preview. So `resolve` cannot ask what a file is called without fetching it, and `check`
//! cannot ask whether it is still there without doing the same. Fetching a file to answer a
//! question about it would spend the person's bandwidth and the account's traffic on a name
//! they already have — the remote job read it out of the folder listing, which is where Seedr
//! does state it. So `resolve` answers with the stable address it was given, and `check`
//! answers `unknown`, which the contract has a word for precisely because "I did not ask" is a
//! different thing from "it is gone".

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LabelPart,
    LinkCheck, LinkStatus, PluginHost, ResolveInput, Resolved,
};
use seedr_common::{address, reason::ErrorEnvelope};

use crate::{api, messages};

/// Most links one `check` call answers. It makes no request, but the answer still crosses the
/// boundary, and an unbounded list would cross it unbounded.
const MAX_CHECKS: usize = 500;

/// Whether this plugin claims `url`. Answered from the address alone and reaching nothing.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    address::claim(url).is_some()
}

/// The hosts this plugin serves, for the account's catalogue.
///
/// Seedr's own and nothing else: it is a hoster for its own storage and unrestricts nobody
/// else's links, so a list naming other hosters would be a promise it cannot keep.
///
/// One entry and not two. The catalogue matches a host or any subdomain of it, so `seedr.cc`
/// already covers `www.seedr.cc` — and it is also the name a person recognises in the account
/// list.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(vec![address::BARE_HOST.to_owned()])
}

/// What the account is, from `GET /rest/user`.
///
/// The one call in this plugin that reaches Seedr, and the only place an account's credential
/// is exercised before a download depends on it — which for a provider whose credential is an
/// e-mail address and a password is worth having.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_password(host, account_id).await?;
    let response = call(host, HttpRequest::get(address::user_url())).await?;
    let record = api::UserRecord::of(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    Ok(Account {
        // The request carried the credential and Seedr answered it: that is what valid means
        // here, and there is nothing else in the document to read it from.
        valid: true,
        // Seedr's own documentation says the REST API is "accessible only to relevant premium
        // account types", so an account that could use it at all is a paying one. A `false`
        // here would say the opposite of what the answer just demonstrated.
        premium: true,
        label: Label::new()
            .user(record.username.as_deref())
            .maybe(space_free(record.space_free()))
            .into(),
        // Deliberately none. `space_max - space_used` is room to *store* into, and reporting it
        // as remaining traffic would tell somebody with a full account that they cannot
        // download from it — which is not true, and is exactly the kind of wrong number an
        // account row is believed on sight.
        traffic_left: None,
    })
}

/// Turns one Seedr address into one download.
///
/// No request. Seedr has no per-file metadata call, so the honest answer is the address itself:
/// it is stable, it is what the remote job wrote, and the name and size a person sees were read
/// out of the folder listing long before this. See the module documentation.
pub(crate) async fn resolve<H: PluginHost>(
    _host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    account(input.account_id.as_deref())?;
    let file_id = address::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_SEEDR_LINK, FailureKind::Unsupported))?;
    Ok(Resolved {
        // Re-built from the identifier rather than passed through, so an address that reached
        // here with a query string or a stray path segment leaves as the canonical one.
        url: address::file_url(file_id),
        // Nothing to add. A name invented out of an identifier would overwrite the one the
        // LinkGrabber already has from Seedr's own folder listing.
        file_name: None,
        size: None,
        // None: a resolver states download headers as *values*, which for HTTP Basic would
        // mean this plugin holding the password, which it never does. The download engine
        // attaches the account's Basic pair itself, because the manifest declares
        // `transfer_auth = "basic"` (RD-120-38).
        headers: Vec::new(),
        checksum: None,
    })
}

/// Whether each of a batch of links is still there.
///
/// `unknown` for every one of them, without a request. Seedr has no call that answers it: the
/// only way to learn whether a file is still in the account is to walk the folder tree to it,
/// which is an unbounded number of requests per link against an account's shared rate limit —
/// or to fetch the file, which is the download itself. `unknown` is what the contract has for
/// exactly this, and reporting `online` without asking would be the worse answer: a person
/// would see a green row for a file somebody deleted last week.
pub(crate) async fn check<H: PluginHost>(
    _host: &H,
    input: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    account(input.account_id.as_deref())?;
    Ok(input
        .urls
        .iter()
        .take(MAX_CHECKS)
        .map(|url| unknown(url))
        .collect())
}

/// Makes one request and turns every answer that is not one into a refusal.
///
/// The whole vocabulary of "Seedr said no" lives here, so no caller decides a second time what
/// a status meant — which is how a rate limit and a rejected password end up under one message.
async fn call<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let response = host
        .http(request.with_header("Authorization", address::AUTHORIZATION_TEMPLATE))
        .await?;
    let envelope = ErrorEnvelope::of(&response.body);
    let retry_after = api::retry_after_seconds(response.header("Retry-After"));
    match api::failure_from(response.status, retry_after, &envelope) {
        None => Ok(response),
        Some(refusal) => {
            let mut failure = Failure::coded(refusal.kind, refusal.code, refusal.message);
            if let Some(reason) = refusal.reason {
                // Sanitised in `seedr_common::reason` before it ever gets here, so an answer
                // that quoted something it should not have keeps nothing.
                failure = failure.with_param("reason", reason);
            }
            Err(failure)
        }
    }
}

/// The account's free storage, as a label part, when Seedr stated both figures.
fn space_free(bytes: Option<u64>) -> Option<LabelPart> {
    let bytes = bytes?;
    Some(
        LabelPart::coded(messages::SPACE_FREE, format!("{bytes} bytes free"))
            .with_param("bytes", bytes.to_string()),
    )
}

/// The account holds a Seedr password, or this fails before a request goes out.
///
/// Asked rather than assumed: without it the host refuses the expansion and the account is told
/// its secret is missing, which is true but says nothing about *which* account or what to do.
async fn require_password<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if host
        .secret_available(account_id, address::PASSWORD_REFERENCE)
        .await
    {
        Ok(())
    } else {
        Err(refuse(
            messages::PASSWORD_MISSING,
            FailureKind::AuthRequired,
        ))
    }
}

fn account(account_id: Option<&str>) -> Result<&str, Failure> {
    account_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| refuse(messages::ACCOUNT_MISSING, FailureKind::AuthRequired))
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}

fn refuse((code, message): (&str, &str), kind: FailureKind) -> Failure {
    Failure::coded(kind, code, message)
}

#[cfg(test)]
mod tests {
    use super::{matches, space_free};

    #[test]
    fn only_seedr_file_addresses_are_claimed() {
        assert!(matches("https://www.seedr.cc/rest/file/42"));
        assert!(matches("https://seedr.cc/rest/file/42"));
        // A magnet is the remote-job sibling's, a folder is not a download, and a stranger's
        // host is nobody's.
        assert!(!matches("magnet:?xt=urn:btih:da39a3ee"));
        assert!(!matches("https://www.seedr.cc/rest/folder/42"));
        assert!(!matches("https://ddownload.com/f/abc"));
    }

    #[test]
    fn free_space_is_a_label_part_only_when_seedr_stated_one() {
        let part = space_free(Some(1024)).expect("a part");
        assert_eq!(part.code, "seedr.space_free");
        assert_eq!(part.params, vec![("bytes".to_owned(), "1024".to_owned())]);
        assert!(space_free(None).is_none());
    }
}
