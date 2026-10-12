//! Choosing among what the repositories offer (RD-1260-02): which offer a core can run, the
//! newest one per plugin, the repository's version of a bundled package, and whether a host
//! answered lately.
//!
//! The index keeps the last version of each plugin per `rdownloader:plugin` contract, so it lists
//! several versions of one plugin that not every core can run. The choice is the client's:
//! [`compatibility_with`] judges an entry for a core, and the newest compatible one wins.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use super::{Offer, PackageCompatibility, PluginRepositoryService, parse_https};
use crate::{
    BundledPackage,
    index::{IndexPackage, Permissions},
    key_fingerprint,
};

/// How long a failed fetch leaves installs from that host to the bundle before it is asked
/// again: without a network every service the setup wizard installs would wait out its own
/// timeout otherwise.
pub const UNREACHABLE_FOR: Duration = Duration::from_secs(300);

/// Whether a core of `app_version` that speaks `contracts` can run `entry`, judged from the index
/// alone. [`compatibility`](super::compatibility) is this for the running build; the preparation
/// of an update asks it for the version the update installs.
#[must_use]
pub fn compatibility_with(
    entry: &IndexPackage,
    contracts: &[&str],
    app_version: &str,
) -> PackageCompatibility {
    if !contracts.contains(&entry.api_version.as_str()) {
        return PackageCompatibility::ContractUnsupported;
    }
    let too_old = entry.min_app_version.as_deref().is_some_and(|minimum| {
        match (
            semver::Version::parse(minimum),
            semver::Version::parse(app_version),
        ) {
            (Ok(minimum), Ok(current)) => current < minimum,
            _ => true,
        }
    });
    if too_old {
        PackageCompatibility::AppTooOld
    } else {
        PackageCompatibility::Compatible
    }
}

fn version(offer: &Offer) -> Option<semver::Version> {
    semver::Version::parse(&offer.entry.version).ok()
}

/// The newest offer this build can run of each plugin, by plugin id. On a tie the one listed
/// first stays, and the official repository is listed first.
#[must_use]
pub fn newest_compatible(offers: &[Offer]) -> HashMap<String, &Offer> {
    let mut newest: HashMap<String, (&Offer, semver::Version)> = HashMap::new();
    for offer in offers {
        if offer.compatibility != PackageCompatibility::Compatible {
            continue;
        }
        let Some(found) = version(offer) else {
            continue;
        };
        let id = offer.entry.id.to_string();
        if newest.get(&id).is_none_or(|(_, known)| found > *known) {
            newest.insert(id, (offer, found));
        }
    }
    newest
        .into_iter()
        .map(|(id, (offer, _))| (id, offer))
        .collect()
}

/// The offer an install of the bundled `package` takes instead of the file beside the program.
///
/// The same plugin as the bundle's — the same id, signed by the same key — at the bundle's version
/// or a newer one, that this build runs and that asks for nothing the bundled package does not:
/// a service installed from the plugin manager or the setup wizard is installed on a click
/// without a preview, so what it is granted has to stay what the release ships. Anything else
/// leaves the install to the bundle, and a newer version that asks for more waits in the plugin
/// manager as an update to review.
#[must_use]
pub fn replacement<'a>(offers: &'a [Offer], package: &BundledPackage) -> Option<&'a Offer> {
    let manifest = &package.manifest;
    let fingerprint = manifest
        .verifying_key()
        .ok()
        .map(|key| key_fingerprint(&key))?;
    let bundled = semver::Version::parse(&manifest.version).ok()?;
    let granted = Permissions::of(manifest);
    let mut best: Option<(&Offer, semver::Version)> = None;
    for offer in offers {
        if offer.compatibility != PackageCompatibility::Compatible
            || offer.entry.id != manifest.id
            || offer.entry.publisher.fingerprint != fingerprint
            || offer.entry.permissions.widens(&granted)
        {
            continue;
        }
        let Some(found) = version(offer).filter(|found| *found >= bundled) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, known)| found > *known) {
            best = Some((offer, found));
        }
    }
    best.map(|(offer, _)| offer)
}

impl PluginRepositoryService {
    /// Whether the host of `url` answered the last fetch, or failed it more than
    /// [`UNREACHABLE_FOR`] ago.
    #[must_use]
    pub fn host_reachable(&self, url: &url::Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        self.0.unreachable.read().ok().is_none_or(|unreachable| {
            unreachable
                .get(host)
                .is_none_or(|until| Instant::now() >= *until)
        })
    }

    /// Whether the package of `offer` can be fetched as far as this service knows: its index is
    /// loaded and its host has not just failed.
    #[must_use]
    pub fn offer_reachable(&self, offer: &Offer) -> bool {
        self.loaded()
            .get(&offer.repository_id)
            .and_then(|index| offer.entry.resolve_url(&index.url).ok())
            .is_some_and(|url| self.host_reachable(&url))
    }

    /// Records how the last fetch from the host of `url` went.
    pub(crate) fn note_reachability(&self, url: &url::Url, reached: bool) {
        let (Some(host), Ok(mut unreachable)) = (url.host_str(), self.0.unreachable.write()) else {
            return;
        };
        if reached {
            unreachable.remove(host);
        } else {
            unreachable.insert(host.to_owned(), Instant::now() + UNREACHABLE_FOR);
        }
    }

    /// Fetches the indexes when none is loaded yet and the official host has not just failed.
    ///
    /// The setup wizard runs before the first scheduled refresh (`STARTUP_DELAY` after the
    /// start), and an install from the repository needs an index. Without a network the refresh
    /// fails, its failure is recorded on the repository, and the installs take the bundle.
    pub async fn ensure_index(&self) {
        if !self.loaded().is_empty() {
            return;
        }
        if parse_https(super::OFFICIAL_INDEX_URL).is_ok_and(|url| !self.host_reachable(&url)) {
            return;
        }
        self.refresh_all().await;
    }

    /// The repository's version of the bundled `package` ([`replacement`]), downloaded and held
    /// to its index entry like every repository package.
    ///
    /// `None` leaves the install to the bundle: no index, no offer that may stand in for the
    /// package, a host that just failed, or a download that fails or does not match. Never an
    /// error — the bundle is the fallback, and a plugin the release ships always installs.
    pub async fn fetch_replacement(&self, package: &BundledPackage) -> Option<(Offer, Vec<u8>)> {
        let offers = match self.offers().await {
            Ok(offers) => offers,
            Err(error) => {
                tracing::warn!(%error, "could not read the repository offers; installing from the bundle");
                return None;
            }
        };
        let offer = replacement(&offers, package)?;
        if !self.offer_reachable(offer) {
            return None;
        }
        let id = offer.entry.id.to_string();
        match self
            .download(&offer.repository_id, &id, &offer.entry.version)
            .await
        {
            Ok(found) => Some(found),
            Err(error) => {
                tracing::warn!(
                    plugin = %offer.entry.name,
                    version = %offer.entry.version,
                    code = error.code(),
                    %error,
                    "the repository's version could not be fetched; installing the bundled one"
                );
                None
            }
        }
    }
}
