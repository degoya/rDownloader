//! The crawl pass of an intake: folder addresses replaced by the files behind them, and the
//! login a protected share needs (RD-104-03, RD-108-07).

use super::links::{CapturedLink, LinkOrigin};
use crate::{ApiError, AppState};

/// Replaces every link an installed crawler claims with the files behind it (RD-104-03).
///
/// Three rules, and each of them is a refusal rather than a feature:
///
/// - **A crawled link is never crawled again.** One pass, no recursion: a crawler that
///   answered with another folder address of its own would otherwise be walked for ever, and
///   the plugin's own walk is where a tree belongs.
/// - **A captured browser request is never crawled.** It is one intercepted download with
///   headers and a body behind it, not a share; expanding it would leave a vaulted body with
///   no owner.
/// - **A folder that produced nothing says so.** An empty or unreachable folder ends as a
///   refusal carrying the plugin's stable code, so the interface can say it in the language
///   the person reads. Silently dropping the link is the defect this replaced.
/// - **A page whose entries wait for a choice resolves nothing here** (RD-1170-03). A
///   two-stage site rule lists a series page's releases on the LinkGrabber's pick board; when
///   that list is all the intake found, it answers `site_rules.pick_waiting` naming the list,
///   so the person -- or the agent -- chooses before a single captcha is asked.
/// - **Nothing a crawler returns is taken on trust.** Every address it hands back passes the
///   verdict in [`crate::collector_crawl_verdict`] before a row exists for it: claimed by a
///   resolver, confirmed as file content by a probe, or refused because the probe answered
///   with a page (RD-110-07). A rule whose pattern reached one element too far used to put
///   the board page itself into the list, and the queue then stored that page under the name
///   of an episode. An address the probe could not reach at all is **kept** -- see that
///   module, and RD-101-06. The probe keeps to the address rule of the find itself
///   ([`LinkOrigin::reach`], RD-150-03): `own_hand` is whether the person handed the intake
///   over themselves.
pub(super) async fn expand_crawled_links(
    state: &AppState,
    links: Vec<CapturedLink>,
    own_hand: bool,
) -> Result<Crawled, ApiError> {
    if state.crawlers.is_empty() {
        return Ok(Crawled::untouched(links));
    }
    let accounts = crawl_accounts(state).await;
    let media_settings = state.media_settings.read().await.clone();
    let gallery_settings = state.gallery_settings.read().await.clone();
    let mut expanded: Vec<CapturedLink> = Vec::with_capacity(links.len());
    let mut refusal: Option<(String, String)> = None;
    let mut listed: Option<(String, rd_plugin_ext::PickSummary)> = None;
    let mut found_total: usize = 0;
    let mut dropped: usize = 0;
    let mut crawled_any = false;
    // One budget for the whole pass, not one per address: this runs inside the intake
    // request, and the per-address timeout alone would let a folder of five hundred stalled
    // links hold it open for hours. What the budget does not reach is kept unproven.
    let probe_deadline = std::time::Instant::now() + crate::collector_crawl_verdict::PROBE_BUDGET;
    // The two rules a find may keep to, built once: listing this machine's addresses is not free.
    let internet = state.scheduler.remote_address_policy(false);
    let local_network = state.scheduler.remote_address_policy(true);
    for link in links {
        if link.request.is_some()
            || link.body_ref.is_some()
            || matches!(link.origin, LinkOrigin::Crawled { .. })
        {
            expanded.push(link);
            continue;
        }
        match state.crawlers.expand(&link.url, &accounts).await {
            rd_plugin_ext::CrawlOutcome::NotClaimed => expanded.push(link),
            rd_plugin_ext::CrawlOutcome::Links(found) => {
                crawled_any = true;
                found_total += found.len();
                // Before the links are kept: the credential a protected share needs, so the
                // queue reaches the files with the same login the crawl read them with.
                if let Err(error) = adopt_share_login(state, &link.url, &found).await {
                    // Named without its cause, which could quote the credential: a share
                    // that ends up without a profile fails visibly at the first download.
                    tracing::warn!(
                        error = %rd_core::redact_text(&error.to_string()),
                        "a protected share's login could not be stored"
                    );
                }
                for crawled in found {
                    if expanded.iter().any(|kept| kept.url == crawled.url) {
                        continue;
                    }
                    let guard = LinkOrigin::Crawled {
                        by_rule: crawled.by_rule,
                    }
                    .reach(own_hand)
                    .map(|local| if local { &local_network } else { &internet });
                    let verdict = crate::collector_crawl_verdict::verdict(
                        state,
                        &crawled.url,
                        &media_settings,
                        &gallery_settings,
                        probe_deadline,
                        guard,
                    )
                    .await;
                    if verdict.keeps_the_link() {
                        if verdict == crate::collector_crawl_verdict::CrawlVerdict::Unconfirmed {
                            tracing::debug!(
                                address = %for_log(&crawled.url),
                                "a crawled address answered nothing and was kept unconfirmed"
                            );
                        }
                        expanded.push(CapturedLink::crawled(crawled));
                        continue;
                    }
                    dropped += 1;
                    if let Some(code) = verdict.code() {
                        tracing::info!(
                            address = %for_log(&crawled.url),
                            code,
                            "a crawled address is not a file and was not added"
                        );
                        refusal
                            .get_or_insert_with(|| (code.to_owned(), verdict.message().to_owned()));
                    }
                }
            }
            rd_plugin_ext::CrawlOutcome::Refused { code, message } => {
                // Without the fragment: that is where a share password rides, and a wrong
                // one is exactly the case this line is written for.
                tracing::info!(address = %for_log(&link.url), %code, "a crawler could not open a folder");
                refusal.get_or_insert((code, message));
            }
            rd_plugin_ext::CrawlOutcome::Listed { rule, page } => {
                tracing::info!(
                    address = %for_log(&link.url),
                    entries = page.entries,
                    "a site rule listed a page's entries for a choice"
                );
                listed.get_or_insert((rule, page));
            }
        }
    }
    let crawled = Crawled {
        links: expanded,
        found: u32::try_from(found_total).unwrap_or(u32::MAX),
        dropped: u32::try_from(dropped).unwrap_or(u32::MAX),
    };
    // A list waiting for a choice is not a failure, but there is no batch to answer with
    // either: the answer names the list instead (RD-1170-03).
    if let Some((rule, page)) = &listed {
        announce_listed(state, rule, page);
    }
    if crawled.links.is_empty()
        && let Some((rule, page)) = listed
    {
        return Err(ApiError::bad_request(
            "site_rules.pick_waiting",
            format!(
                "{rule} listed {} entries; choose which of them to resolve",
                page.entries
            ),
        )
        .with_param("list", page.id)
        .with_param("entries", page.entries)
        .with_param("rule", rule));
    }
    // A refusal is only fatal when nothing else survived: a batch of ten links must not fail
    // over one folder that has gone, and a single folder link that produced nothing must not
    // end as "no links found", which says nothing about what actually happened.
    if crawled.links.is_empty()
        && let Some((code, message)) = refusal
    {
        // The numbers ride along even here, because "the crawler found 40 links and none of
        // them is a file" and "the folder is empty" are different news and used to read the
        // same.
        return Err(ApiError::bad_request_owned(code, message)
            .with_param("found", crawled.found)
            .with_param("dropped", crawled.dropped));
    }
    if crawled_any {
        tracing::info!(
            links = crawled.links.len(),
            found = crawled.found,
            dropped = crawled.dropped,
            "a folder address was expanded"
        );
    }
    Ok(crawled)
}

/// Tells the interface that a page waits for a choice (RD-1190-17), whichever way it came in:
/// a paste in the LinkGrabber hears it from its own answer, but a clipboard copy, the browser
/// extension or Click'n'Load reach the intake through the capture surface, and the interface
/// learnt of their list only at the next paste of its own. Live only, like a captcha signal:
/// the list itself lives in memory and a replay after a restart would name a list that is gone.
fn announce_listed(state: &AppState, rule: &str, page: &rd_plugin_ext::PickSummary) {
    state.database.broadcast(rd_core::EventEnvelope::new(
        rd_core::EventKind::CollectorChanged,
        serde_json::json!({
            "pick_listed": { "list": page.id, "entries": page.entries, "rule": rule }
        }),
    ));
}

/// What the crawl pass produced: the links that survived it, and the count it owes the caller.
///
/// The numbers are the point. A rule whose container pattern greys one element too many used
/// to look like an empty page -- twelve of forty links quietly gone and nothing saying so.
pub(super) struct Crawled {
    pub(super) links: Vec<CapturedLink>,
    /// Addresses the crawlers handed back, before the verdict looked at any of them.
    pub(super) found: u32,
    /// How many of those were refused. `found - dropped` reached the list.
    pub(super) dropped: u32,
}

impl Crawled {
    /// The links of an intake where no crawler ran at all.
    fn untouched(links: Vec<CapturedLink>) -> Self {
        Self {
            links,
            found: 0,
            dropped: 0,
        }
    }
}

/// An address as a log line may carry it: no fragment, and nothing else it hides either.
///
/// A share password rides in the fragment, which is the one part of a URL that never reaches
/// a server -- and would reach a log file, where it outlives the download by months.
fn for_log(url: &url::Url) -> String {
    let mut shown = url.clone();
    shown.set_fragment(None);
    rd_core::redact_url(&shown)
}

/// Longest share password taken from an address, matching what a profile may hold.
const MAX_SHARE_PASSWORD: usize = rd_core::MAX_AUTH_SECRET;

/// Stores the credential a protected share's files need, as an auth profile (RD-108-07).
///
/// **Two decisions live here.** A share password reaches the crawler as the fragment of the
/// pasted address, because `crawl` runs inside the sandbox under a fuel and time budget and
/// has nobody to ask -- and because a fragment is the one part of a URL that is never sent to
/// a server. And a protected share is *not* an account: an account is a per-provider login
/// with a life of its own, while a share password authenticates one share and means nothing
/// anywhere else. What fits it is an auth profile, whose secret is a `vault://` reference and
/// whose scope is a host and a path prefix -- so the queue, which knows nothing about shares,
/// picks the credential up by matching the address it is about to fetch.
///
/// The password is taken from the fragment, encrypted, and never written anywhere else: not
/// into the candidate's address, not into the profile row, not into a REST answer, not into a
/// log line.
async fn adopt_share_login(
    state: &AppState,
    crawled: &url::Url,
    links: &[rd_plugin_ext::CrawledLink],
) -> anyhow::Result<()> {
    let Some(password) = share_password(crawled) else {
        return Ok(());
    };
    let Some(login) = rd_plugin_ext::share_login(links) else {
        return Ok(());
    };
    // The profile of a share crawled before is rewritten rather than duplicated: pasting the
    // same share twice must not leave two credentials behind for one folder.
    let existing = state
        .database
        .list_auth_profiles()
        .await?
        .into_iter()
        .find(|profile| {
            profile.scope == login.scope
                && profile.method == rd_core::AuthMethod::Basic
                && profile.username.as_deref() == Some(login.username.as_str())
        });
    let secret_ref = state.secrets.put_string(password).await?;
    let name = format!(
        "{}{}",
        login.scope.host,
        login.scope.path_prefix.as_deref().unwrap_or("/")
    );
    let orphaned = match existing {
        Some(profile) => {
            let (_, orphaned) = state
                .database
                .update_auth_profile(
                    profile.id,
                    rd_db::UpdateAuthProfile {
                        name: profile.name,
                        scope: login.scope,
                        method: rd_core::AuthMethod::Basic,
                        enabled: true,
                        expires_at: profile.expires_at,
                        username: Some(login.username),
                        secret_ref: Some(secret_ref),
                        certificate_ref: profile.certificate_ref,
                    },
                )
                .await?;
            orphaned
        }
        None => {
            state
                .database
                .create_auth_profile(rd_db::NewAuthProfile {
                    name,
                    scope: login.scope,
                    method: rd_core::AuthMethod::Basic,
                    origin: rd_core::AuthOrigin::Manual,
                    enabled: true,
                    expires_at: None,
                    username: Some(login.username),
                    secret_ref: Some(secret_ref),
                    certificate_ref: None,
                })
                .await?;
            Vec::new()
        }
    };
    for reference in orphaned {
        // Left behind, it is an unreferenced secret in the vault, not a broken package; said,
        // so it can be found (audit 1.9.1, API-13).
        if let Err(error) = state.secrets.remove(&reference).await {
            tracing::warn!(
                error = %format!("{error:#}"),
                "a replaced credential could not be removed from the vault"
            );
        }
    }
    Ok(())
}

/// The share password somebody appended to an address, decoded exactly as the plugin that
/// sent it decodes it.
fn share_password(crawled: &url::Url) -> Option<String> {
    let password = percent_encoding::percent_decode_str(crawled.fragment()?)
        .decode_utf8_lossy()
        .trim()
        .to_owned();
    if password.is_empty() || password.len() > MAX_SHARE_PASSWORD {
        return None;
    }
    Some(password)
}

/// The account each crawler's provider runs as: enabled, and holding a credential.
///
/// Which account a crawl runs as is the application's decision. A plugin naming one would be
/// naming an account it has no business knowing, so it names its provider and gets whatever
/// this finds — or nothing, which is right for a public share.
async fn crawl_accounts(state: &AppState) -> std::collections::HashMap<String, rd_core::AccountId> {
    let providers = state.crawlers.providers();
    if providers.is_empty() {
        return std::collections::HashMap::new();
    }
    let Ok(accounts) = state.database.list_accounts().await else {
        return std::collections::HashMap::new();
    };
    let mut chosen = std::collections::HashMap::new();
    for account in accounts {
        if !account.enabled || !(account.has_secret || account.has_cookies) {
            continue;
        }
        let provider = account.provider.to_ascii_lowercase();
        if providers.contains(&provider) {
            chosen.entry(provider).or_insert(account.id);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::{for_log, share_password};

    /// The fragment of a crawled address is a share password (RD-108-07), and a log line is
    /// the place a credential survives longest. It never goes in one.
    #[test]
    fn a_share_password_never_reaches_a_log_line() {
        let address = url::Url::parse("https://cloud.example.org/s/QxT7bK2mNp9wZr4#s3cret")
            .expect("an address");
        let shown = for_log(&address);
        assert!(!shown.contains("s3cret"), "{shown}");
        assert_eq!(shown, "https://cloud.example.org/s/QxT7bK2mNp9wZr4");
        // Whatever else a URL hides is still redacted; the fragment is only the addition.
        let signed = url::Url::parse("https://cloud.example.org/f?X-Amz-Signature=abc#s3cret")
            .expect("an address");
        let shown = for_log(&signed);
        assert!(
            !shown.contains("s3cret") && !shown.contains("abc"),
            "{shown}"
        );
    }

    #[test]
    fn a_password_is_read_from_the_fragment_the_way_the_plugin_reads_it() {
        let password =
            |address: &str| share_password(&url::Url::parse(address).expect("an address"));
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4#let%20me%20in").as_deref(),
            Some("let me in")
        );
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4"),
            None
        );
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4#"),
            None
        );
        assert_eq!(
            password("https://cloud.example.org/s/QxT7bK2mNp9wZr4#%20%20"),
            None
        );
        let long = "x".repeat(super::MAX_SHARE_PASSWORD + 1);
        assert_eq!(
            password(&format!("https://cloud.example.org/s/Qx#{long}")),
            None
        );
    }
}
