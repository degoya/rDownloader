//! Tests for [`super::carry_over`]: the last version per contract stays, withdrawals do not.

use chrono::{Duration, Utc};
use rd_core::PluginId;

use super::carry_over;
use crate::{
    PluginType,
    index::{
        IndexPackage, PLUGIN_INDEX_SCHEMA_VERSION, Permissions, PluginIndex, Publisher,
        Revocations, RevokedKey,
    },
};

const FIRST: &str = "019d0000-0000-7000-8000-0000000001a1";
const SECOND: &str = "019d0000-0000-7000-8000-0000000001a2";

fn entry(id: &str, version: &str, contract: &str, digest: u8) -> IndexPackage {
    IndexPackage {
        id: id.parse::<PluginId>().expect("id"),
        name: "Fixture".to_owned(),
        version: version.to_owned(),
        plugin_type: PluginType::Resolver,
        api_version: contract.to_owned(),
        min_app_version: Some("1.9.0".to_owned()),
        package_digest: format!("{digest:02x}").repeat(32),
        size: 10,
        url: format!("https://example.test/plugins-{version}/fixture.rdplug"),
        publisher: Publisher {
            key_id: "fixture-v1".to_owned(),
            fingerprint: "ab".repeat(32),
            author: "Fixture Author".to_owned(),
        },
        permissions: Permissions::default(),
        release_notes: None,
    }
}

fn index(packages: Vec<IndexPackage>) -> PluginIndex {
    PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence: 1,
        issued_at: Utc::now() - Duration::days(1),
        not_after: Utc::now() + Duration::days(30),
        packages,
        revoked: Revocations::default(),
    }
}

fn listed(packages: &[IndexPackage]) -> Vec<(String, String, String)> {
    let mut listed: Vec<_> = packages
        .iter()
        .map(|entry| {
            (
                entry.id.to_string(),
                entry.version.clone(),
                entry.api_version.clone(),
            )
        })
        .collect();
    listed.sort();
    listed
}

/// A release on a new contract keeps the last version each plugin had on the old one, and only
/// that one: an installation that has not updated yet still finds what it can run.
#[test]
fn the_last_version_of_each_older_contract_stays() {
    let previous = index(vec![
        entry(FIRST, "1.0.0", "0.10.0", 1),
        entry(FIRST, "1.1.0", "0.10.0", 2),
        entry(SECOND, "2.0.0", "0.10.0", 3),
    ]);
    let mut packages = vec![
        entry(FIRST, "1.2.0", "0.11.0", 4),
        entry(SECOND, "2.1.0", "0.11.0", 5),
    ];
    let kept = carry_over(&previous, &mut packages, &Revocations::default());
    assert_eq!(kept, 2);
    assert_eq!(
        listed(&packages),
        [
            (FIRST.to_owned(), "1.1.0".to_owned(), "0.10.0".to_owned()),
            (FIRST.to_owned(), "1.2.0".to_owned(), "0.11.0".to_owned()),
            (SECOND.to_owned(), "2.0.0".to_owned(), "0.10.0".to_owned()),
            (SECOND.to_owned(), "2.1.0".to_owned(), "0.11.0".to_owned()),
        ]
    );
}

/// On the same contract the new release's version replaces the previous one, and a version the
/// new set lists again is described from its new bytes, not taken over.
#[test]
fn the_same_contract_keeps_only_the_newest_and_the_new_set_wins_a_tie() {
    let previous = index(vec![
        entry(FIRST, "1.1.0", "0.11.0", 1),
        entry(SECOND, "2.0.0", "0.11.0", 2),
    ]);
    let mut packages = vec![
        entry(FIRST, "1.2.0", "0.11.0", 3),
        entry(SECOND, "2.0.0", "0.11.0", 4),
    ];
    assert_eq!(
        carry_over(&previous, &mut packages, &Revocations::default()),
        0
    );
    assert_eq!(packages.len(), 2);
    let second = packages
        .iter()
        .find(|entry| entry.id.to_string() == SECOND)
        .expect("second");
    assert_eq!(second.package_digest, "04".repeat(32));
}

/// A withdrawn package or key never comes back through the previous index.
#[test]
fn withdrawn_entries_are_not_carried_over() {
    let mut foreign = entry(SECOND, "2.0.0", "0.10.0", 3);
    foreign.publisher.fingerprint = "cd".repeat(32);
    let previous = index(vec![entry(FIRST, "1.1.0", "0.10.0", 2), foreign]);
    let revoked = Revocations {
        package_digests: vec!["02".repeat(32)],
        keys: vec![RevokedKey {
            key_id: "other".to_owned(),
            fingerprint: "cd".repeat(32),
        }],
    };
    let mut packages = vec![entry(FIRST, "1.2.0", "0.11.0", 4)];
    assert_eq!(carry_over(&previous, &mut packages, &revoked), 0);
    assert_eq!(packages.len(), 1);
}
