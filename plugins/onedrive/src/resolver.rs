//! OneDrive's protocol logic, written once for both builds.
//!
//! Everything here goes through Microsoft Graph and nothing else: `/shares/{id}/driveItem` for
//! what a sharing link is, `/shares/{id}/items/{item}` and `/drives/{d}/items/{i}` for an item
//! by id, `/content` on any of them for the bytes, `/me/drive` for the account. There is no
//! scraping of the OneDrive web application and no reading of the `authkey` a personal link
//! carries — the sharing link is handed to Graph whole, encoded as Microsoft documents it.
//!
//! The account's access token is never in this file. Requests carry the marker
//! `{{secret:onedrive_access_token}}`, which the host expands on the way out and only
//! towards `graph.microsoft.com`; the sibling OAuth plugin is what puts a value behind it.
//!
//! **The download address is the stable one.** Every `driveItem` also carries a
//! `@microsoft.graph.downloadUrl` — pre-authenticated, valid for about an hour, and the address
//! a quick implementation would hand back. It is deliberately not: an address whose only
//! identity is an expiring signature cannot be asked for again after a restart. The `/content`
//! route can, and that is what `replay::before_resume` does before continuing a partial file
//! — it asks this plugin again, gets the same route, and the transfer's bearer header gets it
//! a fresh redirect for free. Short-lived addresses are renewed without losing progress
//! precisely because nothing short-lived is ever stored.

use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, HttpResponse, Label, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};

use onedrive_common::{address, reason};

use crate::{api, messages, target};

/// The vault reference the OneDrive provider keeps its access token under. The value never
/// reaches this plugin.
const SECRET: &str = "onedrive_access_token";
/// The metadata one resolve needs, and not one field more. The three facets are what say what
/// the item *is*; `deleted` is the tombstone a recycled item still answers with.
const ITEM_FIELDS: &str = "id,name,size,file,folder,package,deleted";
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
        "1drv.ms".to_owned(),
        "onedrive.live.com".to_owned(),
        "sharepoint.com".to_owned(),
    ])
}

/// What the account is, from `/me/drive`.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_token(host, account_id).await?;
    let response = call(
        host,
        HttpRequest::get(format!("{}/me/drive", address::GRAPH))
            .with_query("$select", "id,driveType,owner"),
    )
    .await?;
    let drive = api::drive(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    let user = drive.owner.and_then(|owner| owner.user).unwrap_or_default();
    Ok(Account {
        valid: true,
        // Personal or business, the plugin may do the same things: claiming a tier would be
        // decoration.
        premium: false,
        label: Label::new()
            .user(account_name(
                user.email.as_deref(),
                user.display_name.as_deref(),
            ))
            .into(),
        // Deliberately not the drive's `quota`. That is space left to *upload* into, and
        // reporting it as remaining traffic would tell somebody with a full OneDrive that
        // they cannot download from it — which is not true.
        traffic_left: None,
    })
}

/// Turns one OneDrive address into one download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let claimed = target::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_ONEDRIVE_LINK, FailureKind::Unsupported))?;
    let route = claimed.route();
    let item = fetch_item(host, &route).await?;
    if item.is_deleted() {
        return Err(refuse(messages::ITEM_NOT_FOUND, FailureKind::Permanent));
    }
    if item.is_folder() {
        // The sibling crawler's address, pasted at the resolver. Said plainly rather than as
        // "this item has no bytes", which is what a folder's metadata looks like.
        return Err(refuse(messages::IS_A_FOLDER, FailureKind::Unsupported));
    }
    if !item.is_file() {
        return Err(refuse(messages::NOT_A_FILE, FailureKind::Unsupported));
    }
    Ok(Resolved {
        // The stable route rather than the pre-authenticated `@microsoft.graph.downloadUrl`.
        // That is what makes a resume possible: the scheduler asks this plugin again before
        // continuing a partial file, and an address whose only identity was an expiring
        // signature would have nothing to ask for.
        url: format!("{route}/content"),
        file_name: item.name.clone().filter(|name| !name.is_empty()),
        size: item.size.as_ref().and_then(api::Flexible::as_u64),
        headers: Vec::new(),
        checksum: item.checksum(),
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
        results.push(match fetch_item(host, &claimed.route()).await {
            Ok(item) if item.is_deleted() => offline(url),
            Ok(item) => LinkCheck {
                url: url.clone(),
                status: LinkStatus::Online,
                file_name: item.name.clone().filter(|name| !name.is_empty()),
                size: item.size.as_ref().and_then(api::Flexible::as_u64),
            },
            // An item Graph says is gone is offline; anything else says nothing about the
            // link, so it stays unknown rather than being reported as missing.
            Err(failure) if failure.code.as_deref() == Some(messages::ITEM_NOT_FOUND.0) => {
                offline(url)
            }
            Err(_) => unknown(url),
        });
    }
    Ok(results)
}

/// One item read, with the fields a resolve needs.
async fn fetch_item<H: PluginHost>(host: &H, route: &str) -> Result<api::DriveItem, Failure> {
    let response = call(
        host,
        HttpRequest::get(route.to_owned()).with_query("$select", ITEM_FIELDS),
    )
    .await?;
    api::item(&response.body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// Makes one request and turns every answer that is not one into a refusal.
///
/// The whole vocabulary of "Graph said no" lives here, so no caller decides a second time what
/// a status code meant — which is how a permission and a missing item end up under one message.
async fn call<H: PluginHost>(host: &H, request: HttpRequest) -> Result<HttpResponse, Failure> {
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
    let ((stable, message), kind) = api::classify(response.status, code.as_deref(), retry);
    let mut failure = Failure::coded(kind, stable, message);
    if let Some(code) = code {
        // Sanitised in `reason::of` before it ever gets here, so an error document that quoted
        // a token publishes nothing.
        failure = failure.with_param("reason", code);
    }
    Err(failure)
}

/// Refuses early when the account holds no token at all, rather than making a call that Graph
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
    fn only_onedrive_file_addresses_are_claimed() {
        assert!(matches("https://1drv.ms/u/s!AkXy_Zabc"));
        assert!(matches(
            "https://graph.microsoft.com/v1.0/shares/u!aHR0/items/01ABC"
        ));
        // A folder is the crawler's, and a stranger's host is nobody's.
        assert!(!matches("https://1drv.ms/f/s!AkXy_Zabc"));
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
