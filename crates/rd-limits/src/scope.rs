//! What a limit applies to.

use rd_core::{AccountId, CategoryId, DownloadKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A scope a profile can put its own limit on, next to the global one.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum LimitScope {
    /// One transport (HTTP, Usenet, torrent, media, gallery, stream).
    Protocol(DownloadKind),
    /// One normalised host, without `www.`.
    Host(String),
    /// One provider account.
    Account(AccountId),
    /// One category, and therefore everything routed into it.
    Category(CategoryId),
}

impl LimitScope {
    /// Normalises a host scope so `WWW.Example.COM` and `example.com` are one bucket.
    #[must_use]
    pub fn host(value: &str) -> Self {
        Self::Host(normalize_host(value))
    }
}

/// Where a binding limit came from. Ordered from broadest to narrowest.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LimitSource {
    /// The speed limit set by hand in the Downloads toolbar or the settings; it always
    /// applies, profile or not.
    Manual,
    /// The active profile's global limit.
    Global,
    Protocol,
    Host,
    Account,
    Category,
}

/// The scopes one transfer belongs to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransferScope {
    pub kind: Option<DownloadKind>,
    pub host: Option<String>,
    pub account_id: Option<AccountId>,
    pub category_id: Option<CategoryId>,
}

impl TransferScope {
    /// The scope keys this transfer is subject to, narrowest last.
    #[must_use]
    pub fn keys(&self) -> Vec<(LimitSource, LimitScope)> {
        let mut keys = Vec::new();
        if let Some(kind) = self.kind {
            keys.push((LimitSource::Protocol, LimitScope::Protocol(kind)));
        }
        if let Some(host) = &self.host {
            keys.push((LimitSource::Host, LimitScope::host(host)));
        }
        if let Some(account) = self.account_id {
            keys.push((LimitSource::Account, LimitScope::Account(account)));
        }
        if let Some(category) = self.category_id {
            keys.push((LimitSource::Category, LimitScope::Category(category)));
        }
        keys
    }

    /// Builds the scope of a queued file from its host and identifiers.
    #[must_use]
    pub fn for_download(
        kind: DownloadKind,
        host: Option<&str>,
        account_id: Option<AccountId>,
        category_id: Option<CategoryId>,
    ) -> Self {
        Self {
            kind: Some(kind),
            host: host.map(normalize_host),
            account_id,
            category_id,
        }
    }
}

/// Lower-cases a host and drops a leading `www.`, matching how category rules and the
/// provider registry treat hosts.
#[must_use]
pub fn normalize_host(value: &str) -> String {
    let trimmed = value.trim().to_ascii_lowercase();
    trimmed
        .strip_prefix("www.")
        .map_or(trimmed.clone(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use rd_core::DownloadKind;

    use super::{LimitScope, LimitSource, TransferScope, normalize_host};

    #[test]
    fn hosts_are_normalised_into_one_bucket() {
        assert_eq!(normalize_host(" WWW.Example.COM "), "example.com");
        assert_eq!(
            LimitScope::host("www.example.com"),
            LimitScope::Host("example.com".to_owned())
        );
    }

    #[test]
    fn scope_keys_run_from_broad_to_narrow() {
        let scope = TransferScope {
            kind: Some(DownloadKind::Http),
            host: Some("example.com".to_owned()),
            account_id: Some(rd_core::AccountId::new()),
            category_id: Some(rd_core::CategoryId::new()),
        };
        let sources: Vec<_> = scope.keys().into_iter().map(|(source, _)| source).collect();
        assert_eq!(
            sources,
            vec![
                LimitSource::Protocol,
                LimitSource::Host,
                LimitSource::Account,
                LimitSource::Category
            ]
        );
    }
}
