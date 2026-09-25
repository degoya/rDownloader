//! The trust roots compiled into the binary, and how they rotate.
//!
//! These keys are the bottom of every trust chain the application has: nothing verifies them,
//! so they can only be shipped, never fetched. That is why they live in the binary rather
//! than in a file next to it — a root an attacker can rewrite is not a root.
//!
//! **Rotation is why this is a table and not a constant.** The previous shape was one
//! `&str`, which offers exactly one way to change a key: ship a build that stops trusting
//! everything signed with the old one. Any artefact already published then becomes
//! unverifiable, and any installation that has not updated yet cannot verify the new ones.
//! An overlap window is the only way out, so a role may name more than one key, each with an
//! optional `not_after`; the publisher signs with the new key while the old one is still
//! accepted, and the old entry is dropped a release after it expires.
//!
//! **Roles are separated** so one compromise is not total. A key that signs application
//! updates should not also be able to vouch for a plugin repository index: the update key
//! lives in release CI, the plugin key is used far more often, and giving them one identity
//! makes the more exposed of the two as powerful as the least.

use chrono::{DateTime, Utc};

use crate::trust::TrustStore;

/// What a root key is allowed to vouch for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Application update manifests.
    Release,
    /// Bundled and third-party plugin packages.
    Plugin,
    /// The external-tool manifest and its compatibility rules.
    ToolManifest,
    /// Plugin repository indexes.
    Repository,
    /// The site-rule pack: the rules that recognise release pages (RD-110-04).
    SiteRules,
}

impl Role {
    /// Stable name used in logs and diagnostics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Plugin => "plugin",
            Self::ToolManifest => "tool-manifest",
            Self::Repository => "repository",
            Self::SiteRules => "site-rules",
        }
    }
}

/// One compiled-in public key.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedKey {
    /// What this key may vouch for.
    pub role: Role,
    /// The `key_id` documents signed with it name.
    pub key_id: &'static str,
    /// Base64 Ed25519 public key. An empty string means "not configured in this build".
    pub public_key: &'static str,
    /// RFC 3339 instant after which this key is no longer accepted, if it is being rotated out.
    pub not_after: Option<&'static str>,
}

/// Key id the bundled plugin manifests are signed under.
pub const PLUGIN_RELEASE_KEY_ID: &str = "rdownloader-release-v1";

/// Key id the managed external-tool manifest is signed under (RD-102-02).
pub const TOOL_MANIFEST_KEY_ID: &str = "rdownloader-tools-v1";

/// Key id the shipped site-rule pack is signed under (RD-110-04).
pub const SITE_RULES_KEY_ID: &str = "rdownloader-siterules-v1";

/// Every root this build ships.
///
/// The plugin entry is the key that was previously the lone constant in
/// `crates/rdownloader/src/trusted_keys.rs`; it keeps its id and value, because every
/// `.rdplug` already in the field is signed under it.
///
/// The tool-manifest entry is the root the managed external tools verify against (RD-102-02);
/// the manifest compiled into `rd-tools` is signed under it, and so is any manifest served
/// from a configured URL.
///
/// The site-rules entry is the root the shipped rule pack verifies against (RD-110-04). Its
/// own key, not the tool-manifest one: a rule pack is edited far more often than the tool
/// manifest, and the domain separator keeps a signature from crossing over, but a shared key
/// would still make one compromise vouch for both.
///
/// The release and repository roles carry no key yet: those features publish nothing signed so
/// far, and an empty entry states that honestly instead of inventing a key nobody holds.
/// `rdownloader plugin keygen --role <role>` produces a pair; paste the printed base64 public
/// key here and keep the private PEM as a CI secret.
pub const EMBEDDED_KEYS: &[EmbeddedKey] = &[
    EmbeddedKey {
        role: Role::Plugin,
        key_id: PLUGIN_RELEASE_KEY_ID,
        public_key: "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=",
        not_after: None,
    },
    EmbeddedKey {
        role: Role::Release,
        key_id: "rdownloader-update-v1",
        public_key: "",
        not_after: None,
    },
    EmbeddedKey {
        role: Role::ToolManifest,
        key_id: TOOL_MANIFEST_KEY_ID,
        public_key: "dzbFFZEg9zL7BloYXAu1KfnN+qOBL7MZsYRPZNqwvG4=",
        not_after: None,
    },
    EmbeddedKey {
        role: Role::Repository,
        key_id: "rdownloader-repository-v1",
        public_key: "",
        not_after: None,
    },
    EmbeddedKey {
        role: Role::SiteRules,
        key_id: SITE_RULES_KEY_ID,
        public_key: "QG2IGv/2xPdmiYFHG8pSzV7gmiqh+A9DTDhJxjAjpZI=",
        not_after: None,
    },
];

/// The configured, unexpired keys for one role.
///
/// An entry with an empty `public_key` is skipped rather than reported: a build that was not
/// given a key for a feature nobody has published to yet is not misconfigured.
#[must_use]
pub fn keys_for(role: Role, now: DateTime<Utc>) -> Vec<&'static EmbeddedKey> {
    keys_in(EMBEDDED_KEYS, role, now)
}

/// [`keys_for`] against an arbitrary table, so the rotation window is testable without
/// editing the shipped roots.
fn keys_in(table: &[EmbeddedKey], role: Role, now: DateTime<Utc>) -> Vec<&EmbeddedKey> {
    table
        .iter()
        .filter(|entry| entry.role == role)
        .filter(|entry| !entry.public_key.is_empty())
        .filter(|entry| match entry.not_after {
            Some(text) => DateTime::parse_from_rfc3339(text)
                .map(|expiry| now <= expiry.with_timezone(&Utc))
                // An unparseable expiry means the table is wrong. Refusing the key is the
                // safe reading: a root that cannot be dated cannot be relied on.
                .unwrap_or(false),
            None => true,
        })
        .collect()
}

/// [`keys_for`] at the current instant, for callers with no clock of their own.
#[must_use]
pub fn keys_for_now(role: Role) -> Vec<&'static EmbeddedKey> {
    keys_for(role, Utc::now())
}

/// A trust store holding every configured root for `role`.
pub fn trust_store_for(role: Role, now: DateTime<Utc>) -> anyhow::Result<TrustStore> {
    let store = TrustStore::new();
    for entry in keys_for(role, now) {
        store.trust_base64(entry.key_id.to_owned(), entry.public_key)?;
    }
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_760_000_000, 0).expect("timestamp")
    }

    /// Every configured key has to decode, or the build ships a root nothing can use.
    #[test]
    fn every_configured_root_decodes() {
        for entry in EMBEDDED_KEYS {
            if entry.public_key.is_empty() {
                continue;
            }
            crate::trust::decode_public_key(entry.public_key)
                .unwrap_or_else(|error| panic!("{}: {error}", entry.key_id));
        }
    }

    /// The plugin root must keep its id and value: shipped packages are signed under it.
    #[test]
    fn the_plugin_root_is_unchanged() {
        let keys = keys_for(Role::Plugin, now());
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "rdownloader-release-v1");
        assert_eq!(
            keys[0].public_key,
            "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
        );
    }

    /// The site-rule pack has a root of its own, so the shipped pack verifies against
    /// something that is neither the plugin key nor the tool-manifest key.
    #[test]
    fn the_site_rules_root_is_configured_under_its_own_key() {
        assert_eq!(Role::SiteRules.as_str(), "site-rules");
        let keys = keys_for(Role::SiteRules, now());
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, SITE_RULES_KEY_ID);
        assert_ne!(keys[0].public_key, "");
        for other in [Role::Plugin, Role::ToolManifest] {
            let theirs = keys_for(other, now());
            assert!(
                theirs
                    .iter()
                    .all(|key| key.public_key != keys[0].public_key)
            );
        }
    }

    /// A role with no key configured yields an empty store rather than an error.
    #[test]
    fn a_role_without_a_key_yields_an_empty_store() {
        let store = trust_store_for(Role::Release, now()).expect("store");
        assert!(store.key_ids().expect("ids").is_empty());
    }

    /// Roles must not share a key id, or one role's root would satisfy another's check.
    #[test]
    fn no_key_id_is_used_by_two_roles() {
        let mut seen = std::collections::HashMap::new();
        for entry in EMBEDDED_KEYS {
            if let Some(other) = seen.insert(entry.key_id, entry.role) {
                assert_eq!(other, entry.role, "{} spans two roles", entry.key_id);
            }
        }
    }

    const KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

    fn rotating() -> [EmbeddedKey; 2] {
        [
            EmbeddedKey {
                role: Role::Release,
                key_id: "old",
                public_key: KEY,
                not_after: Some("2020-01-01T00:00:00Z"),
            },
            EmbeddedKey {
                role: Role::Release,
                key_id: "new",
                public_key: KEY,
                not_after: None,
            },
        ]
    }

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("date")
            .with_timezone(&Utc)
    }

    /// During the overlap both keys verify — that is what makes a rotation survivable.
    #[test]
    fn both_keys_are_accepted_inside_the_overlap_window() {
        let table = rotating();
        let keys = keys_in(&table, Role::Release, instant("2019-06-01T00:00:00Z"));
        assert_eq!(keys.len(), 2);
    }

    /// After the window the old root stops being accepted; that is the point of it.
    #[test]
    fn an_expired_key_is_not_returned() {
        let table = rotating();
        let keys = keys_in(&table, Role::Release, instant("2021-01-01T00:00:00Z"));
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "new");
    }

    /// A table entry whose expiry cannot be read is a broken table, and a root that cannot
    /// be dated is not one to rely on.
    #[test]
    fn an_unparseable_expiry_refuses_the_key() {
        let broken = [EmbeddedKey {
            role: Role::Release,
            key_id: "broken",
            public_key: KEY,
            not_after: Some("whenever"),
        }];
        assert!(keys_in(&broken, Role::Release, now()).is_empty());
    }
}
