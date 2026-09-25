//! Dropbox's protocol logic, written once for both builds.
//!
//! Everything here goes through the official Dropbox API v2 and nothing else:
//! `files/get_metadata` for what a file is, `sharing/get_shared_link_metadata` for what a
//! shared link points at, `users/get_current_account` for the account. The bytes come from
//! the content endpoints `files/download` and `sharing/get_shared_link_file`, which the
//! resolver does not call itself: it answers with their **stable** address and the
//! `Dropbox-API-Arg` header that names the file, and the scheduler fetches from there, with
//! the account's token attached because the content host is one of the provider's
//! `secret_domains`. There is no `?dl=1` redirect chasing and no `get_temporary_link`: the
//! first is not the API, and the second would put a one-shot address where a resume needs a
//! stable one (RD-106-04, rule 5).
//!
//! The account's access token is never in this file. Requests carry the marker
//! `{{secret:dropbox_access_token}}`, which the host expands on the way out and only towards
//! `api.dropboxapi.com`; the sibling OAuth plugin is what puts a value behind it.
//!
//! A password-protected shared link is opened the one way the API offers, `link_password` in
//! the official argument. The password comes from the address — `?link_password=` or
//! `?password=` on the pasted link, both redacted by the core — because a resolver sees
//! nothing else of what a person entered.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Header, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};
use serde_json::{Value, json};

use dropbox_common::{api_arg, metadata, reason};

use crate::{
    api, messages,
    target::{self, Target},
};

/// The RPC endpoints, and the only address this plugin reaches itself.
const API: &str = "https://api.dropboxapi.com/2";
/// The content endpoints the transfer goes to. Never called from here.
const CONTENT: &str = "https://content.dropboxapi.com/2";
/// The vault reference the Dropbox provider keeps its access token under. The value never
/// reaches this plugin.
const SECRET: &str = "dropbox_access_token";
/// The name the checksum verifier knows Dropbox's block hash under: SHA-256 over each 4 MiB
/// block, then SHA-256 over the concatenated block digests.
const CONTENT_HASH: &str = "dropbox_content_hash";
/// Most links one `check` call looks up. A check is one request per link, so an unbounded
/// batch is an unbounded number of requests inside one invocation's budget.
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
    Ok(vec![
        "www.dropbox.com".to_owned(),
        "dl.dropboxusercontent.com".to_owned(),
    ])
}

/// What the account is, from `users/get_current_account`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_token(host, account_id).await?;
    let response = call(host, rpc("users/get_current_account", Value::Null)).await?;
    let account = metadata::account(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    Ok(Account {
        valid: true,
        // A Dropbox account is a Dropbox account: no tier changes what this plugin may do, so
        // claiming one would be decoration.
        premium: false,
        label: Label::new()
            .user(account_name(
                account.email.as_deref(),
                account
                    .name
                    .as_ref()
                    .and_then(|name| name.display_name.as_deref()),
            ))
            .into(),
        // Deliberately not `users/get_space_usage`. That is space left to *upload* into, and
        // reporting it as remaining traffic would tell somebody with a full Dropbox that they
        // cannot download from it — which is not true.
        traffic_left: None,
    })
}

/// Turns one Dropbox address into one download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let claimed = target::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_DROPBOX_LINK, FailureKind::Unsupported))?;
    let item = fetch_metadata(host, &claimed).await?;
    if item.is_folder() {
        // The sibling crawler's address, pasted at the resolver. Said plainly rather than as
        // "this file has no bytes", which is what a folder's metadata looks like.
        return Err(refuse(messages::IS_A_FOLDER, FailureKind::Unsupported));
    }
    if !item.is_file() {
        return Err(refuse(messages::FILE_NOT_FOUND, FailureKind::Permanent));
    }
    if !item.downloadable() {
        return Err(refuse(
            messages::DOWNLOAD_NOT_PERMITTED,
            FailureKind::Permanent,
        ));
    }
    let (url, argument) = download(&claimed, &item);
    Ok(Resolved {
        // The stable content address rather than a one-shot URL. That is what makes a resume
        // possible: the scheduler asks this plugin again before continuing a partial file, and
        // an address whose only identity was an expiring token would have nothing to ask for.
        // The file itself is named in the header, regenerated with every answer.
        url,
        file_name: Some(item.name().to_owned()).filter(|name| !name.is_empty()),
        size: item.size,
        headers: vec![Header::new("Dropbox-API-Arg", argument)],
        checksum: item
            .content_hash()
            .map(|hash| (CONTENT_HASH.to_owned(), hash)),
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
        results.push(match fetch_metadata(host, &claimed).await {
            Ok(item) if item.is_file() => LinkCheck {
                url: url.clone(),
                status: LinkStatus::Online,
                file_name: Some(item.name().to_owned()).filter(|name| !name.is_empty()),
                size: item.size,
            },
            Ok(item) if item.is_deleted() => offline(url),
            // A folder says nothing about a file link: it stays unknown.
            Ok(_) => unknown(url),
            // A file Dropbox says is gone is offline; anything else says nothing about the
            // link, so it stays unknown rather than being reported as missing.
            Err(failure) if failure.code.as_deref() == Some(messages::FILE_NOT_FOUND.0) => {
                offline(url)
            }
            Err(_) => unknown(url),
        });
    }
    Ok(results)
}

/// One metadata call, at the endpoint the address belongs to.
async fn fetch_metadata<H: PluginHost>(
    host: &H,
    target: &Target,
) -> Result<metadata::Metadata, Failure> {
    let request = match target {
        Target::Own { path } => rpc("files/get_metadata", json!({ "path": path })),
        Target::Shared { .. } => rpc("sharing/get_shared_link_metadata", shared_argument(target)),
    };
    let response = call(host, request).await?;
    metadata::item(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// The content address the transfer goes to, and the header that names the file there.
fn download(target: &Target, item: &metadata::Metadata) -> (String, String) {
    match target {
        Target::Own { path } => {
            // The revision Dropbox just described, so a transfer never splices two versions of
            // a file that changed while it ran; the id when there is none, the path last.
            let named = match (item.revision(), item.identifier()) {
                (Some(rev), _) => format!("rev:{rev}"),
                (None, Some(id)) => id.to_owned(),
                (None, None) => path.clone(),
            };
            (
                format!("{CONTENT}/files/download"),
                api_arg::ascii_json(&json!({ "path": named })),
            )
        }
        Target::Shared { .. } => (
            format!("{CONTENT}/sharing/get_shared_link_file"),
            api_arg::ascii_json(&shared_argument(target)),
        ),
    }
}

/// The argument both shared-link endpoints take: the link, the path inside it, the password.
fn shared_argument(target: &Target) -> Value {
    let Target::Shared {
        link,
        path,
        password,
    } = target
    else {
        return Value::Null;
    };
    let mut argument = json!({ "url": link });
    if let Some(path) = path {
        argument["path"] = Value::String(path.clone());
    }
    if let Some(password) = password {
        argument["link_password"] = Value::String(password.clone());
    }
    argument
}

/// One RPC request: a JSON body, or `null` for an endpoint that takes no argument.
fn rpc(endpoint: &str, body: Value) -> HttpRequest {
    HttpRequest::post(format!("{API}/{endpoint}"), body.to_string().into_bytes())
        .with_header("Content-Type", "application/json")
}

/// Makes one request and turns every answer that is not one into a refusal.
///
/// The whole vocabulary of "Dropbox said no" lives here, so no caller decides a second time
/// what a status code meant — which is how a password and a permission end up under one
/// message.
async fn call<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let response = host
        .http(request.with_header("Authorization", format!("Bearer {{{{secret:{SECRET}}}}}")))
        .await?;
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    let reason = reason::of(&response.body);
    // Dropbox states the wait twice: as the `Retry-After` header and inside the document.
    let retry = response
        .header("retry-after")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or_else(|| reason::retry_after_in(&response.body));
    let ((code, message), kind) = api::classify(response.status, reason.as_deref(), retry);
    let mut failure = Failure::coded(kind, code, message);
    if let Some(reason) = reason {
        // Sanitised in `reason::of` before it ever gets here, so an error document that quoted
        // a token publishes nothing.
        failure = failure.with_param("reason", reason);
    }
    Err(failure)
}

/// Refuses early when the account holds no token at all, rather than making a call that
/// Dropbox is certain to refuse and reporting whatever it says about it.
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
/// display name only when the address is missing, and nothing at all when both are. The
/// shared label builder bounds it and strips controls, because it is shown rather than parsed.
fn account_name<'a>(email: Option<&'a str>, display_name: Option<&'a str>) -> Option<&'a str> {
    email
        .filter(|value| !value.is_empty())
        .or(display_name.filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::{Label, account_name, matches};

    #[test]
    fn only_dropbox_file_addresses_are_claimed() {
        assert!(matches("https://www.dropbox.com/s/abc123/release.bin?dl=0"));
        assert!(matches(
            "https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1"
        ));
        assert!(matches("https://www.dropbox.com/home/Show?preview=e01.mkv"));
        // A folder is the crawler's, and a stranger's host is nobody's.
        assert!(!matches("https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1"));
        assert!(!matches("https://www.dropbox.com/home/Show"));
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
