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

/// Key id the application update manifests are signed under (RD-180-01).
pub const UPDATE_KEY_ID: &str = "rdownloader-update-v1";

/// Key id the official plugin repository index is signed under (RD-140-01).
pub const REPOSITORY_KEY_ID: &str = "rdownloader-repository-v1";

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
/// Site rules carry no signature since RD-1230-03: they are exchanged as plain export files,
/// and the import shows what a file brings before anything is stored.
///
/// The release entry is the root the update manifests verify against (`rd_update::manifest`,
/// RD-180-01); the repository entry the official plugin index (`rd_plugin_host::index`,
/// RD-140-01). An entry left empty states honestly that a feature publishes nothing signed yet,
/// and [`keys_for`] skips it.
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
        key_id: UPDATE_KEY_ID,
        public_key: "VB5ZJWlQt753fi1fhs7isk7YtjVdmryDnszmqCOhWgA=",
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
        key_id: REPOSITORY_KEY_ID,
        public_key: "NwXtTzLKcfzCuTUAMf20mqYyPvWgmuhZMEJp0UuhuVU=",
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

/// One signed document its publisher withdrew (DB-06), refused whoever signed it.
#[derive(Clone, Copy, Debug)]
pub struct RevokedDocument {
    /// Whose documents it was among.
    pub role: Role,
    /// [`SignedDocument::digest`](crate::SignedDocument::digest) of the withdrawn document,
    /// 64 lowercase hex characters.
    pub digest: &'static str,
}

/// Every signed document this build refuses by its content.
///
/// Compiled in, like the roots, because a withdrawal has to come from the publisher and has to
/// reach an installation the same way its keys do: a list fetched next to the documents could
/// be withheld by whoever serves them. An update manifest, tool manifest or repository index that turns out to be wrong after it was signed goes here with the next
/// release, while its key keeps vouching for everything else. Withdrawing a plugin *package* is
/// the operator's reversible decision and lives elsewhere (`rd_plugin_host::RevokedDigests`,
/// stored by `rd-db`).
pub const REVOKED_DOCUMENTS: &[RevokedDocument] = &[];

/// A trust store holding every configured root for `role`, with that role's withdrawn
/// documents revoked.
pub fn trust_store_for(role: Role, now: DateTime<Utc>) -> anyhow::Result<TrustStore> {
    trust_store_in(EMBEDDED_KEYS, REVOKED_DOCUMENTS, role, now)
}

/// [`trust_store_for`] against arbitrary tables, so the revocation is testable without editing
/// the shipped ones.
fn trust_store_in(
    keys: &[EmbeddedKey],
    revoked: &[RevokedDocument],
    role: Role,
    now: DateTime<Utc>,
) -> anyhow::Result<TrustStore> {
    let store = TrustStore::new();
    for entry in keys_in(keys, role, now) {
        store.trust_base64(entry.key_id.to_owned(), entry.public_key)?;
    }
    for entry in revoked.iter().filter(|entry| entry.role == role) {
        store.revoke_digest(decode_digest(entry.digest)?)?;
    }
    Ok(store)
}

/// A digest from [`REVOKED_DOCUMENTS`]; a malformed one fails the store rather than being
/// skipped, since skipping it would quietly accept the document it withdraws.
fn decode_digest(text: &str) -> anyhow::Result<[u8; 32]> {
    let mut digest = [0_u8; 32];
    anyhow::ensure!(
        text.len() == 64 && text.is_ascii(),
        "a revoked digest is 64 hex characters"
    );
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| anyhow::anyhow!("a revoked digest is 64 hex characters"))?;
    }
    Ok(digest)
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

    /// An entry with no key configured is skipped rather than reported.
    #[test]
    fn an_entry_without_a_key_is_skipped() {
        let unset = [EmbeddedKey {
            role: Role::Release,
            key_id: "unset",
            public_key: "",
            not_after: None,
        }];
        assert!(keys_in(&unset, Role::Release, now()).is_empty());
    }

    /// The update manifests have a root of their own (RD-180-01), shared with no other role.
    #[test]
    fn the_release_root_is_configured_under_its_own_key() {
        assert_eq!(Role::Release.as_str(), "release");
        let keys = keys_for(Role::Release, now());
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, UPDATE_KEY_ID);
        let store = trust_store_for(Role::Release, now()).expect("store");
        assert_eq!(
            store.key_ids().expect("ids"),
            vec![UPDATE_KEY_ID.to_owned()]
        );
        for other in [Role::Plugin, Role::ToolManifest, Role::Repository] {
            assert!(
                keys_for(other, now())
                    .iter()
                    .all(|key| key.public_key != keys[0].public_key)
            );
        }
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

    /// A document in the revocation table is refused by a store built from the roots, while
    /// its key keeps vouching for everything else (DB-06).
    #[test]
    fn a_compiled_in_revocation_refuses_its_document_and_nothing_else() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let public = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            signing.verifying_key().as_bytes(),
        );
        let public: &'static str = Box::leak(public.into_boxed_str());
        let keys = [EmbeddedKey {
            role: Role::Repository,
            key_id: "index",
            public_key: public,
            not_after: None,
        }];
        let sign = |version: &str| {
            crate::sign_document(
                "rdownloader.plugin-index.v1",
                "index",
                &signing,
                &serde_json::json!({ "version": version }),
            )
            .expect("sign")
        };
        let withdrawn = sign("1");
        let current = sign("2");
        let digest: String = withdrawn
            .digest("rdownloader.plugin-index.v1")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let digest: &'static str = Box::leak(digest.into_boxed_str());
        let revoked = [RevokedDocument {
            role: Role::Repository,
            digest,
        }];
        let store = trust_store_in(&keys, &revoked, Role::Repository, now()).expect("store");
        let refused: Result<serde_json::Value, _> =
            withdrawn.verify("rdownloader.plugin-index.v1", &store);
        assert!(matches!(refused, Err(crate::VerifyError::Revoked)));
        let accepted: Result<serde_json::Value, _> =
            current.verify("rdownloader.plugin-index.v1", &store);
        assert!(accepted.is_ok());
        // Another role's store does not carry the revocation.
        let other = trust_store_in(&keys, &revoked, Role::Release, now()).expect("store");
        assert!(
            !other
                .is_revoked_digest(&withdrawn.digest("rdownloader.plugin-index.v1"))
                .expect("read")
        );
    }

    /// A malformed entry fails the store instead of being skipped.
    #[test]
    fn a_malformed_revocation_fails_the_store() {
        let revoked = [RevokedDocument {
            role: Role::Release,
            digest: "not hex",
        }];
        assert!(trust_store_in(EMBEDDED_KEYS, &revoked, Role::Release, now()).is_err());
        for entry in REVOKED_DOCUMENTS {
            decode_digest(entry.digest).unwrap_or_else(|error| panic!("{}: {error}", entry.digest));
        }
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
