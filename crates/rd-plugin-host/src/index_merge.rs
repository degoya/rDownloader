//! Keeping older contracts' packages in the next index (RD-1260-02).
//!
//! The official index is rebuilt with every release over the packages that release signed, and
//! those are built against the release's `rdownloader:plugin` contract. Without the previous
//! entries an installation that has not updated yet — or that updates later, across a contract
//! change — would find nothing it can run. So the publisher carries the previous index's entries
//! over: per plugin, contract and minimum core version the newest one, from either set.

use std::collections::{BTreeMap, BTreeSet};

use rd_core::PluginId;

use super::{IndexPackage, PluginIndex, Revocations};

/// What an entry is kept per: one plugin, built against one contract, for one minimum core.
type Slot = (PluginId, String, Option<String>);

/// Adds to `packages` what `previous` still offers, and keeps per plugin, contract and minimum
/// core version only the newest entry of both.
///
/// A package of the new set wins over a previous entry of the same plugin and version, since
/// the new one was just described from its bytes. A previous entry `revoked` names — by digest or
/// by its publisher's key — is dropped: an index that offers what it withdraws is refused as a
/// whole. Returns how many entries of `previous` stayed.
pub fn carry_over(
    previous: &PluginIndex,
    packages: &mut Vec<IndexPackage>,
    revoked: &Revocations,
) -> usize {
    let withdrawn: BTreeSet<&str> = revoked.package_digests.iter().map(String::as_str).collect();
    let withdrawn_keys: BTreeSet<&str> = revoked
        .keys
        .iter()
        .map(|key| key.fingerprint.as_str())
        .collect();
    let fresh: BTreeSet<(PluginId, String)> = packages
        .iter()
        .map(|entry| (entry.id, entry.version.clone()))
        .collect();
    let carried: Vec<IndexPackage> = previous
        .packages
        .iter()
        .filter(|entry| !fresh.contains(&(entry.id, entry.version.clone())))
        .filter(|entry| !withdrawn.contains(entry.package_digest.as_str()))
        .filter(|entry| !withdrawn_keys.contains(entry.publisher.fingerprint.as_str()))
        .cloned()
        .collect();
    let mut newest: BTreeMap<Slot, (IndexPackage, bool)> = BTreeMap::new();
    let entries = packages
        .drain(..)
        .map(|entry| (entry, false))
        .chain(carried.into_iter().map(|entry| (entry, true)));
    for (entry, from_previous) in entries {
        let slot = (
            entry.id,
            entry.api_version.clone(),
            entry.min_app_version.clone(),
        );
        let replace = newest
            .get(&slot)
            .is_none_or(|(known, _)| version(&entry.version) > version(&known.version));
        if replace {
            newest.insert(slot, (entry, from_previous));
        }
    }
    let mut kept = 0;
    for (entry, from_previous) in newest.into_values() {
        kept += usize::from(from_previous);
        packages.push(entry);
    }
    kept
}

/// A version for ordering; one that is no SemVer sorts below every one that is.
fn version(text: &str) -> Option<semver::Version> {
    semver::Version::parse(text).ok()
}

#[cfg(test)]
#[path = "index_merge_tests.rs"]
mod tests;
