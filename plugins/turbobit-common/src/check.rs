//! The operator's documented link check, `POST links/check`.
//!
//! The one endpoint the site describes as an API (`/linkchecker/api`): `links`, URL-encoded,
//! one per line, answered as a list in the same order with `status` `active`, `inactive` or
//! `invalid`. No key, no account, `X-RateLimit-Limit: 30`. JDownloader sends fifty per call;
//! so does this.

use plugin_common::{CheckInput, Failure, LinkCheck, LinkStatus, PluginHost};
use serde::Deserialize;

use crate::api;
use crate::brand::Brand;
use crate::link;

/// Links per `links/check` call.
const BATCH: usize = 50;

#[derive(Debug, Deserialize)]
struct Entry {
    id: Option<String>,
    name: Option<String>,
    status: Option<String>,
}

/// The batched check.
///
/// # Errors
///
/// See [`api::call`].
pub async fn check<H: PluginHost>(
    brand: &Brand,
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let coded: Vec<(String, Option<String>)> = request
        .urls
        .iter()
        .map(|url| (url.clone(), link::file_id(brand, url)))
        .collect();
    let mut results = Vec::with_capacity(coded.len());
    for chunk in coded.chunks(BATCH) {
        let links: Vec<String> = chunk
            .iter()
            .filter_map(|(_, id)| id.as_deref())
            .map(|id| brand.canonical_link(id))
            .collect();
        let entries: Vec<Entry> = if links.is_empty() {
            Vec::new()
        } else {
            api::call(brand, host, api::links_check(brand, &links), "links/check").await?
        };
        let mut entries = entries.into_iter();
        for (url, id) in chunk {
            let Some(id) = id else {
                results.push(unknown(url));
                continue;
            };
            // Answers come back in the order they were asked; an entry that names another id
            // is not this link's answer, whatever its position.
            let entry = entries
                .next()
                .filter(|entry| entry.id.as_deref().is_none_or(|answered| answered == id));
            results.push(LinkCheck {
                url: url.clone(),
                status: match entry.as_ref().and_then(|entry| entry.status.as_deref()) {
                    Some("active") => LinkStatus::Online,
                    Some("inactive") => LinkStatus::Offline,
                    _ => LinkStatus::Unknown,
                },
                file_name: entry.and_then(|entry| entry.name),
                size: None,
            });
        }
    }
    Ok(results)
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}
