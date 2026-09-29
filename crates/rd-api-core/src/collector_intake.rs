//! The part of the LinkGrabber intake that needs no request: the path a background producer
//! submits links through, the provider every link is routed to, the service switches that refuse
//! one, and the credentials a pasted remote link carries.
//!
//! Shared by the REST intake, the DLC import, the hot folders and the subscriptions, so none of
//! them grows an intake path of its own that could drift from the others.

use rd_db::NewCollectorBatch;

use crate::ApiError;

/// Hands a list of plain URLs to the ordinary intake pipeline.
///
/// The same `add_collector_batch` + online-check path a pasted link takes, so routing rules,
/// categories, grouping and review all apply. Exists so a background producer — a
/// subscription poll (RD-080-07), a feed, an indexer — does not need an `AppState` and does
/// not get an intake path of its own that could drift from this one.
pub(crate) struct PlainIntake<'a> {
    pub database: &'a rd_db::Database,
    pub link_check: &'a crate::link_check_service::LinkCheckService,
    pub media: &'a rd_core::MediaSettings,
    pub gallery: &'a rd_core::GallerySettings,
    pub source: rd_core::IngressSource,
    pub source_label: Option<String>,
    /// Destination for every link in this batch; `None` lets the routing rules decide, as
    /// they do for a pasted link.
    pub category_id: Option<rd_core::CategoryId>,
}

/// A link a background producer submits, with everything its source already knows about it.
///
/// One struct rather than vectors that have to stay parallel: a link dropped by the blocklist
/// used to shift every declared type onto the wrong address, and the name is a second value
/// that must never slide.
pub struct DeclaredLink {
    pub url: url::Url,
    /// What the source says the address is (`application/x-nzb`, …); names the provider
    /// outright instead of leaving it to be guessed from the address.
    pub media_type: Option<String>,
    /// What the source calls it — a feed item's title. An indexer's download address is the
    /// same `…/api` for every hit, so without this the LinkGrabber has nothing to show and
    /// the imported job is called after the API endpoint.
    pub name: Option<String>,
    /// The archive password the source announced, when it announced one (RD-101-17).
    ///
    /// Reaches `collector_packages.password` and from there the extractor, which tries it
    /// before the shared password list. Subscription APIs expose it only to authenticated
    /// clients so the value can be checked when extraction fails.
    pub password: Option<String>,
    /// What the source already declared about the link — an indexer's `<newznab:attr>` block
    /// after the `attributes.rs` gate (RD-107-02).
    ///
    /// Reaches `link_candidates.source_attributes_json` and from there an enricher, so a
    /// plugin asked about a subscription hit does not have to guess the title back out of a
    /// file name. Empty for every producer that declares nothing.
    pub attributes: std::collections::BTreeMap<String, String>,
}

/// Submits links whose kind the caller already knows.
///
/// The declared media type names the provider outright instead of leaving it to be guessed
/// from the address and then re-derived by a HEAD during the online check. An indexer's
/// download address says nothing, and a server that answers a HEAD without a content type —
/// or refuses it — left the link to be fetched as an ordinary file.
pub(crate) async fn submit_plain_links_as(
    intake: PlainIntake<'_>,
    links: Vec<DeclaredLink>,
) -> Result<rd_core::CollectorBatch, ApiError> {
    let PlainIntake {
        database,
        link_check,
        media,
        gallery,
        source,
        source_label,
        category_id,
    } = intake;
    if links.is_empty() {
        return Err(ApiError::bad_request(
            "collector.no_links",
            "No links to submit",
        ));
    }
    let excluded = crate::collector_exclusions::blocklist(database).await?;
    let links: Vec<DeclaredLink> = links
        .into_iter()
        .filter(|link| {
            !link
                .url
                .host_str()
                .is_some_and(|host| crate::collector_exclusions::is_excluded(&excluded, host))
        })
        .collect();
    if links.is_empty() {
        return Err(ApiError::bad_request(
            "collector.all_links_excluded",
            "All links were skipped by the domain blocklist",
        ));
    }
    let urls: Vec<url::Url> = links.iter().map(|link| link.url.clone()).collect();
    let file_names: Vec<Option<String>> = links
        .iter()
        .map(|link| {
            link.name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect();
    let passwords: Vec<Option<String>> = links
        .iter()
        .map(|link| {
            link.password
                .clone()
                .filter(|password| !password.is_empty())
        })
        .collect();
    let source_attributes: Vec<std::collections::BTreeMap<String, String>> =
        links.iter().map(|link| link.attributes.clone()).collect();
    let mut providers = providers_for(&urls, media, gallery);
    for (index, link) in links.iter().enumerate() {
        if let Some(provider) = link
            .media_type
            .as_deref()
            .and_then(rd_core::provider_for_media_type)
            && let Some(slot) = providers.get_mut(index)
        {
            *slot = Some(provider.to_owned());
        }
    }
    let (batch, _, _) = database
        .add_collector_batch(NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source,
            source_label,
            package_name: None,
            // This source carries per-link values. A batch-wide fallback here would make a
            // passwordless sibling inherit the first release's password.
            password: None,
            passwords,
            category_id,
            priority: None,
            providers,
            file_names,
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            urls,
            auto_check: true,
            source_attributes,
        })
        .await?;
    link_check.check_batch(batch.id).await;
    Ok(batch)
}

/// Decides the provider of every link, parallel to `urls`; `None` derives it from the host.
///
/// Whether the service that would carry a link is switched off.
///
/// Keyed on the provider slug `providers_for` derived, so intake and the queue agree on what
/// a link is. WebDAV has no transfer kind of its own — it rides the HTTP engine — so this is
/// the only place it can be refused at all.
pub fn is_service_disabled(
    settings: &crate::dto::SettingsResponse,
    provider: Option<&str>,
) -> bool {
    match provider {
        Some(rd_core::TORRENT_PROVIDER) => !settings.torrent_service_enabled,
        Some(rd_core::NZB_PROVIDER) => !settings.usenet_service_enabled,
        Some(rd_core::MEDIA_PROVIDER) => !settings.media_service_enabled,
        Some(rd_core::GALLERY_PROVIDER) => !settings.gallery_service_enabled,
        Some(
            rd_core::FTP_PROVIDER
            | rd_core::SFTP_PROVIDER
            | rd_core::WEBDAV_PROVIDER
            | rd_core::OBJECT_STORAGE_PROVIDER,
        ) => !settings.remote_service_enabled,
        _ => false,
    }
}

/// Shared with the DLC import, which fills the same batch fields from a container.
pub fn providers_for(
    urls: &[url::Url],
    media: &rd_core::MediaSettings,
    gallery: &rd_core::GallerySettings,
) -> Vec<Option<String>> {
    // Media hosts win over gallery hosts when both lists name the same site.
    urls.iter()
        .map(|url| {
            if url.scheme() == "magnet" || url.path().ends_with(".torrent") {
                return Some(rd_core::TORRENT_PROVIDER.to_owned());
            }
            // The transfer protocols are decided by the scheme alone: the address names a
            // specific server, so there is nothing to infer from the host.
            if rd_core::ObjectStorageProvider::from_scheme(url.scheme()).is_some() {
                return Some(rd_core::OBJECT_STORAGE_PROVIDER.to_owned());
            }
            if let Some((protocol, _)) = rd_core::RemoteProtocol::from_url_scheme(url.scheme()) {
                return Some(
                    match protocol.family() {
                        rd_core::RemoteFamily::Ftp => rd_core::FTP_PROVIDER,
                        rd_core::RemoteFamily::Sftp => rd_core::SFTP_PROVIDER,
                        rd_core::RemoteFamily::Webdav => rd_core::WEBDAV_PROVIDER,
                    }
                    .to_owned(),
                );
            }
            // An NZB is imported, not downloaded. Saving the document into the download
            // folder and stopping there is the wrong outcome, and it is what happened
            // before this was routed (RD-080-11). An indexer link that hides the extension
            // behind a query is reclassified by the online check, which sees the type.
            if url.path().to_ascii_lowercase().ends_with(".nzb") {
                return Some(rd_core::NZB_PROVIDER.to_owned());
            }
            // A direct manifest is a media source regardless of the host it sits on: a CDN
            // that serves `.m3u8` is never in the media-host list and never will be
            // (RD-080-06). A URL with no telling extension is reclassified by the online
            // check, which sees the content type.
            let path = url.path().to_ascii_lowercase();
            if path.ends_with(".m3u8") || path.ends_with(".mpd") {
                return Some(rd_core::MEDIA_PROVIDER.to_owned());
            }
            let host = url.host_str()?;
            if media.handles_host(host) {
                Some(rd_core::MEDIA_PROVIDER.to_owned())
            } else if gallery.handles_host(host) {
                Some(rd_core::GALLERY_PROVIDER.to_owned())
            } else {
                None
            }
        })
        .collect()
}

/// Takes the credentials out of pasted `ftp://user:pw@host/…` links.
///
/// Such a link is the ordinary way people share an FTP location, so it is accepted rather
/// than rejected — but the password must not survive into the candidate row, the queue, an
/// SSE event or a log line. The credential is stored once, the link is rewritten to its
/// bare form, and everything downstream only ever sees the sanitised URL.
///
/// An existing login for the same endpoint and user wins; a pasted password never silently
/// overwrites one that was configured deliberately.
pub async fn adopt_remote_credentials(
    database: &rd_db::Database,
    secrets: &rd_secrets::SecretStore,
    urls: &mut [url::Url],
) -> Result<(), ApiError> {
    for url in urls.iter_mut() {
        // An object storage link authenticates through its profile, so a key pair pasted
        // into it is dropped rather than adopted: that is not where anybody should keep one,
        // and it must not reach the candidate row either.
        if rd_core::ObjectStorageProvider::from_scheme(url.scheme()).is_some() {
            // Also for a link the parser refuses: it is still stored as a candidate.
            let _ = url.set_password(None);
            if let Some(canonical) = rd_core::ObjectAddress::parse(url).and_then(|a| a.url()) {
                *url = canonical;
            }
            continue;
        }
        let Some(target) = rd_core::RemoteTarget::parse(url) else {
            continue;
        };
        // WebDAV authenticates through auth profiles, so it has no login to adopt.
        if target.protocol.family() == rd_core::RemoteFamily::Webdav {
            continue;
        }
        let password = url.password().map(str::to_owned);
        if let Some(sanitized) = target.sanitized_url() {
            *url = sanitized;
        }
        let Some(username) = target.username.clone() else {
            continue;
        };
        if database.match_remote_credential(&target).await?.is_some() {
            continue;
        }
        let secret_ref = match password.filter(|value| !value.is_empty()) {
            Some(password) => Some(secrets.put_string(password).await?),
            None => None,
        };
        let auth_mode = if secret_ref.is_some() {
            rd_core::RemoteAuthMode::Password
        } else {
            rd_core::RemoteAuthMode::Anonymous
        };
        let created = database
            .create_remote_credential(rd_db::NewRemoteCredential {
                name: format!("{}@{}", username, target.host),
                protocol: target.protocol,
                host: target.host.clone(),
                port: target.port,
                username: Some(username),
                auth_mode,
                passive: true,
                enabled: true,
                secret_ref: secret_ref.clone(),
                key_ref: None,
                passphrase_ref: None,
            })
            .await;
        if created.is_err() {
            crate::config_fields::cleanup_secrets(secrets, [secret_ref]).await;
        }
    }
    Ok(())
}
