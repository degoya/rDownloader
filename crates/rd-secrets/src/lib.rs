//! Encrypted, reference-based local secret storage.
//!
//! The master key is held in the OS keyring wherever one is reachable. Where it is not, it
//! falls back to a file beside the vault: created 0600 on Unix, but on Windows created with
//! nothing but the ACL it inherits from its directory, so another local user who can read
//! that directory can read the master key. Closing that gap needs a Win32 ACL call, which
//! means a dependency this crate does not have and an `unsafe` block the workspace denies.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::Rng;
/// What [`SecretStore::get`] hands back and how a caller reads it, for crates that keep no
/// `secrecy` dependency of their own.
pub use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const KEYRING_SERVICE: &str = "rDownloader";
const KEYRING_USER: &str = "master-key";
const REFERENCE_PREFIX: &str = "vault://";
const AAD: &[u8] = b"rDownloader secret v1";

/// Cloneable encrypted vault. The master key never leaves process memory unencrypted.
#[derive(Clone)]
pub struct SecretStore {
    root: Arc<PathBuf>,
    key: Arc<[u8; 32]>,
}

#[derive(Deserialize, Serialize)]
struct Envelope {
    version: u8,
    nonce: String,
    ciphertext: String,
}

impl SecretStore {
    /// Opens the service's vault and obtains its master key from the OS keyring or a fallback
    /// file (0600 on Unix, directory-inherited permissions on Windows -- see the module docs).
    ///
    /// The keyring entry is one per user account, not one per vault, so this is for the vault
    /// of the running service only. Everything else -- above all a test -- opens with
    /// [`SecretStore::open`], which never touches the keyring: on a CI runner a locked macOS
    /// keychain blocked that call indefinitely, and on a developer's machine a test would read
    /// or mint the master key of the real installation.
    pub async fn open_with_os_keyring(root: PathBuf) -> Result<Self> {
        Self::open_inner(root, true).await
    }

    /// Opens a vault whose master key lives only in the file beside it, created on first use.
    pub async fn open(root: PathBuf) -> Result<Self> {
        Self::open_inner(root, false).await
    }

    async fn open_inner(root: PathBuf, os_keyring: bool) -> Result<Self> {
        tokio::fs::create_dir_all(&root)
            .await
            .with_context(|| format!("create secret directory {}", root.display()))?;
        let key = load_or_create_master_key(&root, os_keyring).await?;
        Ok(Self {
            root: Arc::new(root),
            key: Arc::new(key),
        })
    }

    /// Encrypts a secret and returns an opaque reference suitable for SQLite metadata.
    pub async fn put(&self, secret: SecretString) -> Result<String> {
        let reference = Self::new_reference();
        self.put_at(&reference, secret).await?;
        Ok(reference)
    }

    /// A fresh reference nothing is stored under yet (RD-190-04).
    ///
    /// For a caller that has to record the reference *before* the value exists — the archive
    /// passwords reserve theirs in the database first, so a stop between the two writes leaves
    /// a reservation the next start removes rather than a file nothing points at.
    #[must_use]
    pub fn new_reference() -> String {
        format!("{REFERENCE_PREFIX}{}", Uuid::now_v7())
    }

    /// Encrypts a secret under a reference from [`SecretStore::new_reference`], replacing what
    /// was stored under it. Writing the same value twice is harmless, which is what lets an
    /// interrupted caller simply repeat the write.
    pub async fn put_at(&self, reference: &str, secret: SecretString) -> Result<()> {
        if secret.expose_secret().is_empty() {
            bail!("refuse to store an empty secret");
        }
        let id = parse_reference(reference)?;
        let mut nonce_bytes = [0_u8; 24];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_slice())
            .map_err(|_| anyhow::anyhow!("invalid vault master key"))?;
        let ciphertext = cipher
            .encrypt(
                &XNonce::from(nonce_bytes),
                Payload {
                    msg: secret.expose_secret().as_bytes(),
                    aad: AAD,
                },
            )
            .map_err(|_| anyhow::anyhow!("encrypt secret"))?;
        let envelope = Envelope {
            version: 1,
            nonce: STANDARD.encode(nonce_bytes),
            ciphertext: STANDARD.encode(ciphertext),
        };
        self.write_atomic(id, serde_json::to_vec(&envelope)?).await
    }

    /// Convenience boundary for callers that just received a request string.
    pub async fn put_string(&self, secret: String) -> Result<String> {
        self.put(SecretString::from(secret)).await
    }

    /// Removes an encrypted value after its owning metadata was rejected.
    ///
    /// A temporary a stopped [`SecretStore::put_at`] left under the same reference goes too.
    pub async fn remove(&self, reference: &str) -> Result<()> {
        let id = parse_reference(reference)?;
        remove_if_present(&self.temporary_path(id))
            .await
            .context("remove a temporary secret")?;
        remove_if_present(&self.secret_path(id))
            .await
            .context("remove encrypted secret")
    }

    /// Resolves and decrypts one vault reference.
    pub async fn get(&self, reference: &str) -> Result<SecretString> {
        let id = parse_reference(reference)?;
        let bytes = tokio::fs::read(self.secret_path(id))
            .await
            .context("read encrypted secret")?;
        let envelope: Envelope =
            serde_json::from_slice(&bytes).context("decode secret envelope")?;
        if envelope.version != 1 {
            bail!("unsupported secret envelope version");
        }
        let nonce = STANDARD
            .decode(envelope.nonce)
            .context("decode secret nonce")?;
        let ciphertext = STANDARD
            .decode(envelope.ciphertext)
            .context("decode secret ciphertext")?;
        if nonce.len() != 24 {
            bail!("invalid secret nonce");
        }
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_slice())
            .map_err(|_| anyhow::anyhow!("invalid vault master key"))?;
        let plaintext = cipher
            .decrypt(
                <&XNonce>::try_from(nonce.as_slice())
                    .map_err(|_| anyhow::anyhow!("invalid secret nonce"))?,
                Payload {
                    msg: &ciphertext,
                    aad: AAD,
                },
            )
            .map_err(|_| anyhow::anyhow!("decrypt secret"))?;
        String::from_utf8(plaintext)
            .map(SecretString::from)
            .context("secret is not UTF-8")
    }

    /// Puts raw key material away and returns the reference it is reached by (RD-110-33).
    ///
    /// A content transform's key arrives from a plugin as bytes, and bytes are what the
    /// cipher needs back -- but the vault stores text, and going through a lossy conversion
    /// would silently corrupt a key with a byte no UTF-8 sequence produces. Base64 both ways,
    /// in one reviewed place, so no caller invents its own encoding.
    pub async fn put_bytes(&self, bytes: &[u8]) -> Result<String> {
        if bytes.is_empty() {
            bail!("refuse to store empty key material");
        }
        self.put(SecretString::from(STANDARD.encode(bytes))).await
    }

    /// Resolves one reference written by [`SecretStore::put_bytes`].
    pub async fn get_bytes(&self, reference: &str) -> Result<Vec<u8>> {
        let encoded = self.get(reference).await?;
        STANDARD
            .decode(encoded.expose_secret())
            .context("stored key material is not base64")
    }

    async fn write_atomic(&self, id: Uuid, content: Vec<u8>) -> Result<()> {
        let destination = self.secret_path(id);
        let temporary = self.temporary_path(id);
        // A write of the same reference that stopped before its rename left this behind, and
        // the temporary is created exclusively.
        remove_if_present(&temporary)
            .await
            .context("remove a stale temporary secret")?;
        write_private(&temporary, &content).await?;
        tokio::fs::rename(&temporary, &destination)
            .await
            .context("commit encrypted secret")?;
        // The rename is only durable once the folder is (DB-08): a power loss right after it
        // could otherwise bring back the old value, or none, under a reference already recorded.
        sync_directory(&self.root).await;
        Ok(())
    }

    /// Every reference the vault holds a value for, or a temporary a stopped write left behind
    /// under (DB-03), in no particular order. The master key is not one.
    ///
    /// # Errors
    ///
    /// When the folder cannot be read.
    pub async fn stored_references(&self) -> Result<Vec<String>> {
        let mut entries = tokio::fs::read_dir(self.root.as_path())
            .await
            .context("read the secret directory")?;
        let mut references = std::collections::BTreeSet::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .context("read the secret directory")?
        {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let id = name.strip_suffix(".secret").or_else(|| {
                name.strip_prefix('.')
                    .and_then(|rest| rest.strip_suffix(".tmp"))
            });
            if let Some(id) = id.and_then(|id| Uuid::parse_str(id).ok()) {
                references.insert(format!("{REFERENCE_PREFIX}{id}"));
            }
        }
        Ok(references.into_iter().collect())
    }

    fn secret_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.secret"))
    }

    fn temporary_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!(".{id}.tmp"))
    }
}

async fn remove_if_present(path: &std::path::Path) -> std::io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Every vault reference written anywhere in `text` -- a column of its own or a field of a JSON
/// document -- in the canonical spelling [`SecretStore::put`] hands out.
#[must_use]
pub fn references_in(text: &str) -> Vec<String> {
    const UUID_LENGTH: usize = 36;
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(REFERENCE_PREFIX) {
        let after = &rest[start + REFERENCE_PREFIX.len()..];
        if let Some(id) = after
            .get(..UUID_LENGTH)
            .and_then(|candidate| Uuid::parse_str(candidate).ok())
        {
            found.push(format!("{REFERENCE_PREFIX}{id}"));
        }
        rest = after;
    }
    found
}

/// The id of a reference, in the one spelling [`SecretStore::new_reference`] produces: lower
/// case and hyphenated (RA-DB-07). `Uuid::parse_str` also takes the simple, braced and URN
/// forms and upper case, which [`references_in`] does not find as written; a value stored under
/// one of them would look orphaned to the sweep at start and be removed.
fn parse_reference(reference: &str) -> Result<Uuid> {
    let value = reference
        .strip_prefix(REFERENCE_PREFIX)
        .context("unsupported secret reference")?;
    let id = Uuid::parse_str(value).context("invalid secret reference")?;
    if id.to_string() != value {
        bail!("secret reference not in its canonical form");
    }
    Ok(id)
}

async fn load_or_create_master_key(root: &std::path::Path, os_keyring: bool) -> Result<[u8; 32]> {
    if os_keyring && let Some(key) = load_keyring_key()? {
        return Ok(key);
    }
    let fallback = root.join("master.key");
    if let Ok(bytes) = tokio::fs::read(&fallback).await {
        return bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid fallback master key length"));
    }
    let mut key = [0_u8; 32];
    rand::rng().fill_bytes(&mut key);
    let encoded = STANDARD.encode(key);
    let stored_in_keyring = os_keyring
        && keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .and_then(|entry| entry.set_password(&encoded))
            .is_ok();
    if !stored_in_keyring {
        write_private(&fallback, &key).await?;
        // A key file that vanished with a power loss after secrets were written under it would
        // leave all of them undecryptable (DB-08).
        sync_directory(root).await;
    }
    Ok(key)
}

/// Makes a new or renamed entry in `directory` durable ([`rd_files::durable::sync_directory`]),
/// off the runtime's threads.
async fn sync_directory(directory: &std::path::Path) {
    let directory = directory.to_path_buf();
    let _ =
        tokio::task::spawn_blocking(move || rd_files::durable::sync_directory(&directory)).await;
}

fn load_keyring_key() -> Result<Option<[u8; 32]>> {
    // Constructing the entry fails when the platform has no credential store at all -- a
    // headless Linux install with no session D-Bus is the common case -- and such an install
    // has to keep starting, so it falls through to the file fallback. Reading the entry is
    // deliberately not treated the same way: see below.
    let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) else {
        return Ok(None);
    };
    let encoded = match entry.get_password() {
        Ok(encoded) => encoded,
        // The one error that really means "never set, or deleted".
        Err(keyring::Error::NoEntry) => return Ok(None),
        // A locked login session, or a keyring daemon that is not up yet, reports a readable
        // entry as unreadable. Treating that as "no key stored yet" would make the caller mint
        // a fresh master key and overwrite the stored one, leaving every account password,
        // NNTP credential and TOTP seed in the vault permanently undecryptable. Fail startup
        // instead.
        Err(error) => return Err(error).context("read vault master key from the OS keyring"),
    };
    let bytes = STANDARD
        .decode(encoded)
        .context("decode keyring master key")?;
    Ok(Some(bytes.try_into().map_err(|_| {
        anyhow::anyhow!("invalid keyring master key length")
    })?))
}

#[cfg(unix)]
async fn write_private(path: &std::path::Path, content: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true).mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(content).await?;
    file.sync_data().await?;
    Ok(())
}

// `create_new` for the same reason as the Unix path: a plain write truncates, and the one file
// this is used for outside the temporary is the fallback `master.key`. Replacing a key that is
// already in use leaves every account password, NNTP credential and TOTP seed in the vault
// permanently undecryptable, so an existing file has to be an error rather than a target.
//
// "private" is aspirational on this path: no DACL is set, so the file is readable by whoever
// the parent directory grants read access to. Restricting it to the current user needs
// `SetNamedSecurityInfo` from a Win32 binding, which is a dependency this crate does not carry.
#[cfg(not(unix))]
async fn write_private(path: &std::path::Path, content: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await?;
    file.write_all(content).await?;
    file.sync_data().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SecretStore, references_in, write_private};

    #[test]
    fn references_are_found_in_columns_and_documents() {
        let column = "vault://0199a000-0000-7000-8000-000000000001";
        let document = r#"{"secret_ref":"vault://0199a000-0000-7000-8000-000000000002","x":"vault://not-a-uuid"}"#;
        assert_eq!(references_in(column), vec![column.to_owned()]);
        assert_eq!(
            references_in(document),
            vec!["vault://0199a000-0000-7000-8000-000000000002".to_owned()]
        );
        assert!(references_in("no reference here").is_empty());
    }

    /// RA-DB-07: only the spelling `new_reference` produces is a reference. Every other form
    /// of the same id would be stored under it and then missed by `references_in`.
    #[tokio::test]
    async fn a_reference_in_another_spelling_of_its_id_is_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = SecretStore::open(directory.path().to_owned())
            .await
            .expect("store");
        let canonical = SecretStore::new_reference();
        let id = canonical.trim_start_matches("vault://");
        for other in [
            format!("vault://{}", id.to_uppercase()),
            format!("vault://{}", id.replace('-', "")),
            format!("vault://{{{id}}}"),
            format!("vault://urn:uuid:{id}"),
        ] {
            assert!(
                store
                    .put_at(&other, secrecy::SecretString::from("value".to_owned()))
                    .await
                    .is_err(),
                "{other} is refused"
            );
            assert!(store.get(&other).await.is_err(), "{other} reads nothing");
        }
        assert!(store.stored_references().await.expect("list").is_empty());
        store
            .put_at(&canonical, secrecy::SecretString::from("value".to_owned()))
            .await
            .expect("the canonical form is stored");
        assert_eq!(references_in(&canonical), vec![canonical.clone()]);
    }

    /// The listing names what `put` wrote and a stopped write's temporary, never the master key.
    #[tokio::test]
    async fn stored_references_list_values_and_temporaries() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = SecretStore::open(directory.path().to_owned())
            .await
            .expect("store");
        let written = store.put_string("value".to_owned()).await.expect("put");
        let stopped = SecretStore::new_reference();
        let id = stopped.trim_start_matches("vault://");
        std::fs::write(directory.path().join(format!(".{id}.tmp")), b"x").expect("temporary");
        let mut expected = vec![written, stopped];
        expected.sort();
        assert_eq!(store.stored_references().await.expect("list"), expected);
    }
    use secrecy::ExposeSecret;

    /// `open` keeps its master key beside the vault and nowhere else. Where an OS keyring is
    /// reachable the old `open` put the key there instead and wrote no file — and it did so for
    /// every test's vault, in the one entry the real installation uses.
    #[tokio::test]
    async fn open_keeps_the_master_key_in_the_file_beside_the_vault() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = SecretStore::open(directory.path().to_owned())
            .await
            .expect("store");
        let key = std::fs::read(directory.path().join("master.key")).expect("master key file");
        assert_eq!(key.as_slice(), store.key.as_slice());
    }

    #[tokio::test]
    async fn encrypted_secret_round_trips_and_is_not_plaintext() {
        let directory = tempfile::tempdir().expect("tempdir");
        write_private(&directory.path().join("master.key"), &[7_u8; 32])
            .await
            .expect("master key");
        let store = SecretStore::open(directory.path().to_owned())
            .await
            .expect("store");
        let reference = store
            .put("very-secret-value".to_owned().into())
            .await
            .expect("put");
        let id = reference.trim_start_matches("vault://");
        let envelope = std::fs::read_to_string(directory.path().join(format!("{id}.secret")))
            .expect("envelope");
        assert!(!envelope.contains("very-secret-value"));
        assert_eq!(
            store.get(&reference).await.expect("get").expose_secret(),
            "very-secret-value"
        );
    }

    /// A reference recorded before its value exists (RD-190-04): the value lands under exactly
    /// that reference, a repeated write replaces it, and a temporary a stopped write left
    /// behind neither blocks the repeat nor outlives the removal.
    #[tokio::test]
    async fn a_reserved_reference_is_written_repeated_and_removed_whole() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = SecretStore::open(directory.path().to_owned())
            .await
            .expect("store");
        let reference = SecretStore::new_reference();
        assert!(reference.starts_with("vault://"));
        store
            .get(&reference)
            .await
            .expect_err("nothing is stored under a fresh reference");
        let id = reference.trim_start_matches("vault://");
        let temporary = directory.path().join(format!(".{id}.tmp"));
        std::fs::write(&temporary, b"left by a stopped write").expect("stale temporary");
        store
            .put_at(&reference, "first".to_owned().into())
            .await
            .expect("put");
        store
            .put_at(&reference, "second".to_owned().into())
            .await
            .expect("repeat");
        assert_eq!(
            store.get(&reference).await.expect("get").expose_secret(),
            "second"
        );
        std::fs::write(&temporary, b"left by a stopped write").expect("stale temporary");
        store.remove(&reference).await.expect("remove");
        assert!(!temporary.exists());
        assert!(!directory.path().join(format!("{id}.secret")).exists());
        store
            .remove(&reference)
            .await
            .expect("a second removal finds nothing");
        store
            .put_at("vault://not-a-uuid", "value".to_owned().into())
            .await
            .expect_err("a reference the vault did not mint");
    }

    /// Key material is bytes, and a byte that is not UTF-8 must survive the round trip
    /// (RD-110-33). Storing it as text without an encoding is how a key quietly becomes a
    /// different key.
    #[tokio::test]
    async fn key_material_survives_the_round_trip_byte_for_byte() {
        let directory = tempfile::tempdir().expect("tempdir");
        write_private(&directory.path().join("master.key"), &[7_u8; 32])
            .await
            .expect("master key");
        let store = SecretStore::open(directory.path().to_owned())
            .await
            .expect("store");
        let key = [
            0x0c, 0x4c, 0x44, 0xe1, 0x28, 0xea, 0xee, 0x7a, 0x40, 0xbc, 0xbd, 0x4f, 0xfe, 0xc1,
            0x96, 0x17,
        ];
        let reference = store.put_bytes(&key).await.expect("put");
        assert!(reference.starts_with("vault://"));
        assert_eq!(store.get_bytes(&reference).await.expect("get"), key);
        // Nothing of the key is on disk in the clear.
        let id = reference.trim_start_matches("vault://");
        let envelope = std::fs::read_to_string(directory.path().join(format!("{id}.secret")))
            .expect("envelope");
        let encoded = base64::Engine::encode(&super::STANDARD, key);
        assert!(!envelope.contains(&encoded), "{envelope}");
        store.put_bytes(&[]).await.expect_err("empty key material");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_master_key_file_is_not_group_or_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("master.key");
        write_private(&path, &[7_u8; 32]).await.expect("master key");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // A second write must not silently replace a master key that is already in use.
        write_private(&path, &[9_u8; 32])
            .await
            .expect_err("existing master key overwritten");
    }
}
