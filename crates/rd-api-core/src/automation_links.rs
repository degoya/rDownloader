//! The `add_links` automation action (RD-1240-10): links into the LinkGrabber or straight into
//! the downloads, each by the path a person's own links take.
//!
//! The LinkGrabber half is the plain intake the subscriptions use, so routing rules, the
//! blocklist, the online check and the review apply exactly as to a pasted link. The downloads
//! half is what `POST /api/v1/downloads` does for one HTTP(S) address: a package of its own,
//! named after the file, in the default destination, with the account that covers its hoster.
//!
//! **Address rule (RD-150-03, coordinator 2026-10-10).** An automation runs unattended and again
//! and again, long after somebody wrote its links, so they are not checked as a pasted link is:
//! they take the reach of `LinkOrigin::Proposed` from an intake by the person's own hand, as the
//! hot folder's `.rdlinks` import does -- the person's own network, never this machine. In the
//! LinkGrabber the candidates are held to it before the online check starts, which marks a link
//! to this machine `collector.check_internal_address` and never requests it; for the downloads a
//! literal address on this machine is refused before anything is queued, and every download is
//! written with that reach, so the transfer keeps to it for a name that resolves there too.

use rd_automation::LinkDestination;

use crate::automation_actions::ActionContext;

/// The reach of an automation's links: `LinkOrigin::Proposed.reach(own_hand = true)`, the
/// person's own network and never this machine.
const AUTOMATION_REACH_LOCAL_NETWORK: bool = true;

/// What the LinkGrabber half needs beyond the action context.
#[derive(Clone)]
pub struct LinkIntake {
    pub link_check: crate::link_check_service::LinkCheckService,
    pub media_settings: rd_media::SharedMediaSettings,
    pub gallery_settings: rd_gallery::SharedGallerySettings,
}

/// Hands the action's links to their destination. `Err` is retryable, as for every action; a
/// retry of the downloads half adds only the links the failed attempt did not.
pub(crate) async fn add_links(
    context: &ActionContext,
    links: &[String],
    destination: LinkDestination,
) -> anyhow::Result<()> {
    let urls = links
        .iter()
        .map(|link| url::Url::parse(link.trim()).map(rd_collector::canonical_url))
        .collect::<Result<Vec<_>, _>>()?;
    match destination {
        LinkDestination::LinkGrabber => to_linkgrabber(context, urls).await,
        LinkDestination::Downloads => to_downloads(context, urls).await,
    }
}

async fn to_linkgrabber(context: &ActionContext, urls: Vec<url::Url>) -> anyhow::Result<()> {
    let Some(intake) = &context.links else {
        anyhow::bail!("the LinkGrabber intake is not available");
    };
    let media = intake.media_settings.read().await.clone();
    let gallery = intake.gallery_settings.read().await.clone();
    let links = urls
        .into_iter()
        .map(|url| crate::collector_intake::DeclaredLink {
            url,
            media_type: None,
            name: None,
            password: None,
            attributes: std::collections::BTreeMap::new(),
        })
        .collect();
    crate::collector_intake::submit_plain_links_as(
        crate::collector_intake::PlainIntake {
            database: &context.database,
            link_check: &intake.link_check,
            media: &media,
            gallery: &gallery,
            source: rd_core::IngressSource::Api,
            source_label: Some("automation".to_owned()),
            category_id: None,
            reach: Some(AUTOMATION_REACH_LOCAL_NETWORK),
        },
        links,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?;
    Ok(())
}

async fn to_downloads(context: &ActionContext, urls: Vec<url::Url>) -> anyhow::Result<()> {
    let destination = crate::destination::resolve_destination(&context.database, None).await?;
    let accounts = context.database.list_accounts().await.unwrap_or_default();
    let queued: std::collections::HashSet<String> = context
        .database
        .list_downloads()
        .await?
        .into_iter()
        .map(|file| file.source.to_string())
        .collect();
    // Every link is judged before the first is queued, so a refused one leaves nothing half done.
    let policy = context
        .scheduler
        .remote_address_policy(AUTOMATION_REACH_LOCAL_NETWORK);
    for url in &urls {
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "only HTTP(S) links go straight to the downloads"
        );
        anyhow::ensure!(
            policy.hop_refusal(url).is_none(),
            "{}: an automation's link may not point at this machine",
            rd_core::CODE_INTERNAL_ADDRESS
        );
    }
    for url in urls {
        // A retry after a partial failure must not add the links that already went in.
        if queued.contains(url.as_str()) {
            continue;
        }
        let file_name = url
            .path_segments()
            .and_then(Iterator::last)
            .filter(|value| !value.is_empty())
            .map_or_else(
                || rd_files::FALLBACK_FILE_NAME.to_owned(),
                rd_files::decode_path_segment,
            );
        let package_name = context
            .database
            .tidy_package_name(&rd_files::package_name_from_file_name(&file_name), None)
            .await?;
        let account_id = account_for(context, &accounts, &url).await;
        let options = rd_scheduler::PackageOptions {
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            paused: false,
            address_reach: Some(AUTOMATION_REACH_LOCAL_NETWORK),
        };
        match &destination {
            Some(directory) => {
                context
                    .scheduler
                    .enqueue_direct_to_with_network(
                        url,
                        package_name,
                        file_name,
                        directory.clone(),
                        account_id,
                        None,
                        options,
                    )
                    .await?;
            }
            None => {
                context
                    .scheduler
                    .enqueue_direct_with_network(
                        url,
                        package_name,
                        file_name,
                        account_id,
                        None,
                        options,
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

/// The first enabled account whose catalogue covers the link's hoster, as a direct download
/// over REST picks it (`hosters::fallback_account`).
async fn account_for(
    context: &ActionContext,
    accounts: &[rd_core::Account],
    url: &url::Url,
) -> Option<rd_core::AccountId> {
    let resolvers = context.scheduler.resolvers();
    for account in accounts.iter().filter(|account| account.enabled) {
        if crate::hosters::supports(
            &crate::hosters::catalogue(&resolvers, account.id).await,
            url,
        ) {
            return Some(account.id);
        }
    }
    None
}
