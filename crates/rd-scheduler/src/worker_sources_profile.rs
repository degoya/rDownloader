//! The authentication profile of a mirror's own host (RD-1320-05).
//!
//! A download's profile — the one it is pinned to, or the one its own address matched — is
//! chosen for that address, and its headers stay inside the profile's scope (RD-120-43). A
//! mirror on another host therefore got no profile at all, though one for that host may well
//! exist. It now gets the profile its own address matches, as if it had been added on its own:
//! the download's pinned profile applies to the download's own host only, and a download that
//! asked for no profile sends none anywhere.
//!
//! The mirrors share the download's one client, so the mirror's profile rides per request: a
//! Basic or Bearer profile as its `Authorization`, a cookie profile as the `Cookie` header its
//! cookies make for that address. reqwest drops both when a redirect changes host, and the
//! address a probe ends at is asked again.

use anyhow::Result;
use rd_core::{AuthMethod, AuthProfileSelection, DownloadFile};
use reqwest::cookie::{CookieStore, Jar};
use secrecy::ExposeSecret;
use url::Url;

use crate::SchedulerHandle;

/// The headers of the profile `target` matches, for a mirror on a host other than the
/// download's own. Empty for the download's own host — its own client covers that — for a
/// download that asked for no profile, for the download's own profile, and for a header
/// `already` carries.
pub(super) async fn mirror_profile_headers(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    target: &Url,
    already: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    if file.auth_profile == AuthProfileSelection::None || same_host(&file.source, target) {
        return Ok(Vec::new());
    }
    let Some(profile) = scheduler.database.match_auth_profile(target).await? else {
        return Ok(Vec::new());
    };
    let own = match file.auth_profile {
        AuthProfileSelection::Pinned(id) => id == profile.id,
        // Matched for the download's own address too, it is the download's own profile.
        _ => profile.scope.matches_url(&file.source),
    };
    if own {
        return Ok(Vec::new());
    }
    let Some(reference) = profile.secret_ref.as_deref() else {
        return Ok(Vec::new());
    };
    let carries = |name: &str| {
        already
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(name))
    };
    let secret = scheduler.secrets.get(reference).await?;
    match profile.method {
        AuthMethod::Basic | AuthMethod::Bearer if !carries("authorization") => Ok(vec![(
            "authorization".to_owned(),
            super::super::client::authorization_value(&profile, &secret)?,
        )]),
        AuthMethod::Cookies if !carries("cookie") => {
            let jar = Jar::default();
            let scope = rd_http::CookieScope::new(
                &profile.scope.probe_url().unwrap_or_else(|| target.clone()),
                profile.scope.include_subdomains,
            )?;
            rd_http::import_into(&jar, secret.expose_secret(), &scope)?;
            Ok(jar
                .cookies(target)
                .and_then(|value| value.to_str().ok().map(str::to_owned))
                .map(|value| vec![("cookie".to_owned(), value)])
                .unwrap_or_default())
        }
        _ => Ok(Vec::new()),
    }
}

fn same_host(left: &Url, right: &Url) -> bool {
    left.host_str()
        .zip(right.host_str())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
}

#[cfg(test)]
#[path = "worker_sources_profile_tests.rs"]
mod tests;
