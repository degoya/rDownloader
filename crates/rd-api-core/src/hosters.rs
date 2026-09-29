//! Per-account hoster catalogues served by the account's resolver, plus the automatic
//! account fallback for links whose own hoster has no account configured.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::AccountId;
use rd_plugin_host::ResolverService;
use url::Url;

use crate::{ApiError, AppState, dto::AccountHostersResponse};

const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

struct Entry {
    fetched_at: Instant,
    hosters: Vec<String>,
}

static CATALOGUES: OnceLock<Mutex<HashMap<AccountId, Entry>>> = OnceLock::new();

fn cached(account_id: AccountId, allow_stale: bool) -> Option<Vec<String>> {
    let lock = CATALOGUES.get_or_init(|| Mutex::new(HashMap::new()));
    let guard = lock.lock().ok()?;
    let entry = guard.get(&account_id)?;
    (allow_stale || entry.fetched_at.elapsed() < CACHE_TTL).then(|| entry.hosters.clone())
}

fn remember(account_id: AccountId, hosters: &[String]) {
    let lock = CATALOGUES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = lock.lock() {
        guard.insert(
            account_id,
            Entry {
                fetched_at: Instant::now(),
                hosters: hosters.to_vec(),
            },
        );
    }
}

/// Drops the cached catalogue of an account (after edits or deletion).
pub fn forget(account_id: AccountId) {
    if let Some(mut guard) = CATALOGUES.get().and_then(|lock| lock.lock().ok()) {
        guard.remove(&account_id);
    }
}

/// Hoster domains the account can download from, cached for an hour per account.
/// Resolver failures log a warning and fall back to the last known list (or nothing).
pub async fn catalogue(resolvers: &ResolverService, account_id: AccountId) -> Vec<String> {
    if let Some(hosters) = cached(account_id, false) {
        return hosters;
    }
    match resolvers.hosters(account_id).await {
        Ok(hosters) => {
            remember(account_id, &hosters);
            hosters
        }
        Err(error) => {
            tracing::warn!(%account_id, error = %error.message, "hoster catalogue unavailable");
            cached(account_id, true).unwrap_or_default()
        }
    }
}

/// Whether `url` points at one of the catalogue's hosters (or a subdomain of one).
#[must_use]
pub fn supports(hosters: &[String], url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host
        .strip_prefix("www.")
        .unwrap_or(host)
        .to_ascii_lowercase();
    hosters.iter().any(|hoster| {
        let hoster = hoster.to_ascii_lowercase();
        host == hoster || host.ends_with(&format!(".{hoster}"))
    })
}

/// Picks the first enabled account whose catalogue covers the link's hoster.
pub async fn fallback_account(state: &AppState, url: &Url) -> Option<AccountId> {
    let accounts = state.database.list_accounts().await.ok()?;
    let resolvers = state.scheduler.resolvers();
    for account in accounts.into_iter().filter(|account| account.enabled) {
        if supports(&catalogue(&resolvers, account.id).await, url) {
            return Some(account.id);
        }
    }
    None
}

#[utoipa::path(get, path = "/api/v1/accounts/{id}/hosters", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = AccountHostersResponse), (status = 404)))]
pub async fn list_account_hosters(
    State(state): State<AppState>,
    Path(id): Path<AccountId>,
) -> Result<Json<AccountHostersResponse>, ApiError> {
    let account = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .find(|account| account.id == id)
        .ok_or_else(crate::error_codes::account_not_found)?;
    Ok(Json(AccountHostersResponse {
        account_id: account.id,
        provider: account.provider,
        hosters: catalogue(&state.scheduler.resolvers(), id).await,
    }))
}

#[cfg(test)]
mod tests {
    use super::supports;

    #[test]
    fn matches_hoster_and_subdomains_case_insensitively() {
        let hosters = vec!["rapidgator.net".to_owned(), "ddownload.com".to_owned()];
        let url = |value: &str| value.parse().expect("URL");
        assert!(supports(
            &hosters,
            &url("https://www.RapidGator.net/file/abc")
        ));
        assert!(supports(&hosters, &url("https://cdn7.ddownload.com/x")));
        assert!(!supports(&hosters, &url("https://notrapidgator.net/file")));
        assert!(!supports(&hosters, &url("https://example.com/file")));
    }
}
