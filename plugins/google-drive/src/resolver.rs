//! Google Drive's protocol logic, written once for both builds.
//!
//! Everything here goes through the official Drive v3 API and nothing else: `files.get` for
//! what a file is, `files.get?alt=media` for its bytes, `files.export` for a Workspace
//! document, `about` for the account. There is no scraping of the download interstitial and no
//! `confirm=` token guessing — those are the tricks that break, and the ones the job's
//! cross-cutting requirement rules out.
//!
//! The account's access token is never in this file. Requests carry the marker
//! `{{secret:google_drive_access_token}}`, which the host expands on the way out and only
//! towards `www.googleapis.com`; the sibling OAuth plugin is what puts a value behind it.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};

use google_drive_common::{address, export, reason};

use crate::{api, messages, target};

/// The Drive v3 API, and the only address this plugin reaches.
const API: &str = "https://www.googleapis.com/drive/v3";
/// The vault reference the Google Drive provider keeps its access token under. The value never
/// reaches this plugin.
const SECRET: &str = "google_drive_access_token";
/// The metadata one resolve needs, and not one field more. `fields` is not an optimisation
/// here: Drive answers a `files.get` without it with a partial record that has no size in it.
const FILE_FIELDS: &str =
    "id,name,mimeType,size,md5Checksum,sha256Checksum,capabilities(canDownload),trashed";
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
        "drive.google.com".to_owned(),
        "docs.google.com".to_owned(),
        "drive.usercontent.google.com".to_owned(),
    ])
}

/// What the account is, from `about`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_token(host, account_id).await?;
    let response = call(
        host,
        HttpRequest::get(format!("{API}/about"))
            .with_query("fields", "user(displayName,emailAddress)"),
    )
    .await?;
    let about = api::about(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    let user = about.user.unwrap_or(api::AboutUser {
        email_address: None,
        display_name: None,
    });
    Ok(Account {
        valid: true,
        // A Drive account is a Drive account: there is no premium tier that changes what this
        // plugin may do, so claiming one would be decoration.
        premium: false,
        label: Label::new()
            .user(account_name(
                user.email_address.as_deref(),
                user.display_name.as_deref(),
            ))
            .into(),
        // Deliberately not `storageQuota`. That is space left to *upload* into, and reporting
        // it as remaining traffic would tell somebody with a full Drive that they cannot
        // download from it — which is not true and is exactly the kind of wrong number an
        // account row is believed on sight.
        traffic_left: None,
    })
}

/// Turns one Drive address into one download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let claimed = target::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_DRIVE_LINK, FailureKind::Unsupported))?;
    let metadata = fetch_metadata(host, &claimed.id).await?;
    if metadata.trashed == Some(true) {
        return Err(refuse(messages::FILE_NOT_FOUND, FailureKind::Permanent));
    }
    let mime = metadata.mime_type.clone().unwrap_or_default();
    if export::is_folder(&mime) {
        // The sibling crawler's address, pasted at the resolver. Said plainly rather than as
        // "this file has no bytes", which is what a folder's metadata looks like.
        return Err(refuse(messages::IS_A_FOLDER, FailureKind::Unsupported));
    }
    if !metadata.can_download() {
        return Err(refuse(
            messages::DOWNLOAD_NOT_PERMITTED,
            FailureKind::Permanent,
        ));
    }
    let name = metadata.name.clone().unwrap_or_default();
    if export::is_workspace_document(&mime) {
        let chosen = export::resolve(&mime, claimed.format.as_deref()).map_err(|refusal| {
            refuse(
                match refusal {
                    export::Refusal::UnsupportedType => messages::EXPORT_UNSUPPORTED,
                    export::Refusal::UnsupportedFormat => messages::EXPORT_FORMAT_UNSUPPORTED,
                },
                FailureKind::Unsupported,
            )
        })?;
        return Ok(Resolved {
            url: format!(
                "{API}/files/{}/export?mimeType={}",
                claimed.id,
                address::percent_encode(chosen.mime)
            ),
            file_name: Some(export::export_name(&name, &chosen)),
            // An export has no size and no checksum until it has run: the bytes do not exist
            // yet. Stating a number here would be inventing one.
            size: None,
            headers: Vec::new(),
            checksum: None,
        });
    }
    Ok(Resolved {
        // The stable API address rather than a one-shot URL. That is what makes a resume
        // possible: the scheduler asks this plugin again before continuing a partial file, and
        // an address whose only identity was an expiring token would have nothing to ask for.
        url: format!(
            "{API}/files/{}?alt=media&supportsAllDrives=true",
            claimed.id
        ),
        file_name: Some(name).filter(|name| !name.is_empty()),
        size: metadata.size.as_ref().and_then(api::Flexible::as_u64),
        headers: Vec::new(),
        checksum: metadata.checksum(),
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
        results.push(match fetch_metadata(host, &claimed.id).await {
            Ok(metadata) if metadata.trashed == Some(true) => LinkCheck {
                url: url.clone(),
                status: LinkStatus::Offline,
                file_name: None,
                size: None,
            },
            Ok(metadata) => {
                let mime = metadata.mime_type.clone().unwrap_or_default();
                let name = metadata.name.clone().unwrap_or_default();
                // The name a Workspace document will actually arrive under, decided here so
                // it is visible in the LinkGrabber before anything is queued.
                let file_name = if export::is_workspace_document(&mime) {
                    export::resolve(&mime, claimed.format.as_deref())
                        .ok()
                        .map(|chosen| export::export_name(&name, &chosen))
                } else {
                    Some(name).filter(|name| !name.is_empty())
                };
                LinkCheck {
                    url: url.clone(),
                    status: LinkStatus::Online,
                    file_name,
                    size: metadata.size.as_ref().and_then(api::Flexible::as_u64),
                }
            }
            // A file Drive says is gone is offline; anything else says nothing about the link,
            // so it stays unknown rather than being reported as missing.
            Err(failure) if failure.code.as_deref() == Some(messages::FILE_NOT_FOUND.0) => {
                LinkCheck {
                    url: url.clone(),
                    status: LinkStatus::Offline,
                    file_name: None,
                    size: None,
                }
            }
            Err(_) => unknown(url),
        });
    }
    Ok(results)
}

/// One `files.get`, with the fields a resolve needs.
async fn fetch_metadata<H: PluginHost>(host: &H, id: &str) -> Result<api::FileMetadata, Failure> {
    let response = call(
        host,
        HttpRequest::get(format!("{API}/files/{id}"))
            .with_query("fields", FILE_FIELDS)
            .with_query("supportsAllDrives", "true"),
    )
    .await?;
    api::file(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// Makes one request and turns every answer that is not one into a refusal.
///
/// The whole vocabulary of "Drive said no" lives here, so no caller decides a second time what
/// a status code meant — which is how a quota and a permission end up under one message.
async fn call<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
    let response = host
        .http(request.with_header("Authorization", format!("Bearer {{{{secret:{SECRET}}}}}")))
        .await?;
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    let reason = reason::of(&response.body);
    let retry = response
        .header("retry-after")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let ((code, message), kind) = api::classify(response.status, reason.as_deref(), retry);
    let mut failure = Failure::coded(kind, code, message);
    if let Some(reason) = reason {
        // Sanitised in `api::reason` before it ever gets here, so an error document that quoted
        // a token publishes nothing.
        failure = failure.with_param("reason", reason);
    }
    Err(failure)
}

/// Refuses early when the account holds no token at all, rather than making a call that Google
/// is certain to refuse and reporting whatever it says about it.
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
    fn only_drive_file_addresses_are_claimed() {
        assert!(matches("https://drive.google.com/file/d/1A2b3C/view"));
        assert!(matches(
            "https://www.googleapis.com/drive/v3/files/1A2b3C?alt=media"
        ));
        // A folder is the crawler's, and a stranger's host is nobody's.
        assert!(!matches("https://drive.google.com/drive/folders/1A2b3C"));
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
