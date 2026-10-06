//! The link fragment that is key material, from intake to deletion (RD-110-38).
//!
//! Two accepted decisions collided here. RD-109-32 strips the fragment off every address
//! before a candidate row exists, because nothing distinguishes a share password from an
//! anchor name; ADR 0011 was accepted on a file key that travels in exactly that fragment.
//! The resolution is that the fragment goes into the vault: the stored address is shortened
//! exactly as before, and what was cut off is reachable only through a reference.
//!
//! **This file is the canary.** The public example key below is searched for in every row of
//! every table, in every event payload, and in the JSON a candidate is served as. A fragment
//! that survived anywhere is a decryption key in a LinkGrabber row, which is the leak
//! RD-109-32 closed and the acceptance criterion RD-103-02 states in as many words.

use rd_core::IngressSource;
use rd_db::{Database, NewCollectorBatch};

/// The public example from somebody else's README, the same one `plugins/mega` is tested
/// with. No private link and no real account is in this tree.
const KEY: &str = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
const MEGA: &str = "https://mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
/// A link whose provider declared nothing: its fragment is dropped, as it always was.
const OTHER: &str = "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret";

/// The declaration a manifest makes. Set for the whole test process; the table is global
/// because the installed plugin set is.
fn declare_mega() {
    rd_provider_registry::replace_secret_fragment_hosts(vec![
        "mega.nz".to_owned(),
        "mega.co.nz".to_owned(),
    ]);
}

async fn open(directory: &std::path::Path, with_vault: bool) -> Database {
    let database = Database::open(directory.join("collector.sqlite3"))
        .await
        .expect("database");
    if with_vault {
        let vault = rd_secrets::SecretStore::open(directory.join("secrets"))
            .await
            .expect("vault");
        database.install_secret_vault(vault);
    }
    database
}

fn batch(urls: Vec<&str>) -> NewCollectorBatch {
    NewCollectorBatch {
        source: IngressSource::Clipboard,
        source_label: None,
        package_name: None,
        password: None,
        passwords: Vec::new(),
        category_id: None,
        priority: None,
        urls: urls
            .into_iter()
            .map(|url| url.parse().expect("URL"))
            .collect(),
        providers: Vec::new(),
        file_names: Vec::new(),
        sizes: Vec::new(),
        package_hints: Vec::new(),
        mirror_hints: Vec::new(),
        requests: Vec::new(),
        body_refs: Vec::new(),
        auto_check: false,
        source_attributes: Vec::new(),
    }
}

/// Every byte the database file and its write-ahead log hold.
///
/// Blunter than reading the columns this job added, and deliberately so: a canary that named
/// the tables it checks would miss the next column somebody adds, and missing one is the only
/// way this can fail. The vault lives in a directory of its own and is not read here.
async fn every_stored_byte(database: &Database, directory: &std::path::Path) -> Vec<u8> {
    database.checkpoint_wal().await.expect("checkpoint");
    let mut bytes = Vec::new();
    for entry in std::fs::read_dir(directory).expect("read directory") {
        let path = entry.expect("entry").path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("collector.sqlite3"))
        {
            bytes.extend(std::fs::read(&path).expect("read database file"));
        }
    }
    bytes
}

/// Whether `needle` occurs anywhere in `haystack`.
fn holds(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test]
async fn a_declared_link_loses_its_fragment_from_the_row_and_keeps_it_in_the_vault() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![MEGA]))
        .await
        .expect("batch");
    let candidate = &candidates[0];
    assert_eq!(candidate.url.as_str(), "https://mega.nz/file/yuZ0QJ6J");
    assert!(
        candidate.secret_fragment,
        "the candidate must say that a key is being kept for it"
    );
    let reference = database
        .candidate_secret_fragment_ref(candidate.id)
        .await
        .expect("read")
        .expect("a reference");
    assert!(
        reference.starts_with("vault://"),
        "the row carries a reference, not a secret: {reference}"
    );
    let vault = database.secret_vault().expect("a vault");
    assert_eq!(
        String::from_utf8(vault.get_bytes(&reference).await.expect("get")).expect("utf-8"),
        KEY
    );
}

#[tokio::test]
async fn a_link_no_provider_declared_behaves_exactly_as_it_did_before() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![OTHER]))
        .await
        .expect("batch");
    let candidate = &candidates[0];
    assert_eq!(
        candidate.url.as_str(),
        "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4"
    );
    assert!(!candidate.secret_fragment);
    assert_eq!(
        database
            .candidate_secret_fragment_ref(candidate.id)
            .await
            .expect("read"),
        None
    );
    assert!(
        !holds(
            &every_stored_byte(&database, directory.path()).await,
            "s3cret"
        ),
        "an undeclared fragment must still be dropped outright"
    );
}

/// Without a vault the fragment is dropped, exactly as it was before this job existed. A key
/// nothing can read back would be worse than none, and `rdownloader doctor` runs like this.
#[tokio::test]
async fn without_a_vault_the_fragment_is_dropped_rather_than_stored() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), false).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![MEGA]))
        .await
        .expect("batch");
    assert_eq!(candidates[0].url.as_str(), "https://mega.nz/file/yuZ0QJ6J");
    assert!(!candidates[0].secret_fragment);
    assert!(!holds(
        &every_stored_byte(&database, directory.path()).await,
        KEY
    ));
}

#[tokio::test]
async fn deleting_a_candidate_takes_its_secret_with_it() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![MEGA]))
        .await
        .expect("batch");
    let reference = database
        .candidate_secret_fragment_ref(candidates[0].id)
        .await
        .expect("read")
        .expect("a reference");
    let vault = database.secret_vault().expect("a vault").clone();
    assert!(vault.get_bytes(&reference).await.is_ok(), "stored first");

    database
        .delete_candidate(candidates[0].id)
        .await
        .expect("delete");

    assert!(
        vault.get_bytes(&reference).await.is_err(),
        "a secret whose owner is gone is a leak with a delay"
    );
}

/// The same for the two bulk paths, because a leak does not care which button removed the row.
#[tokio::test]
async fn clearing_the_linkgrabber_takes_every_secret_with_it() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![MEGA]))
        .await
        .expect("batch");
    let reference = database
        .candidate_secret_fragment_ref(candidates[0].id)
        .await
        .expect("read")
        .expect("a reference");
    let vault = database.secret_vault().expect("a vault").clone();

    let removed = database.delete_candidates().await.expect("delete all");
    assert_eq!(removed, 1);
    assert!(vault.get_bytes(&reference).await.is_err());
}

#[tokio::test]
async fn deleting_a_linkgrabber_package_takes_its_secrets_with_it() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, packages, candidates) = database
        .add_collector_batch(batch(vec![MEGA]))
        .await
        .expect("batch");
    let reference = database
        .candidate_secret_fragment_ref(candidates[0].id)
        .await
        .expect("read")
        .expect("a reference");
    let vault = database.secret_vault().expect("a vault").clone();

    database
        .delete_collector_package(packages[0].id)
        .await
        .expect("delete package");
    assert!(vault.get_bytes(&reference).await.is_err());
}

/// **The canary.** Nothing the database holds, publishes or serves carries the fragment.
#[tokio::test]
async fn no_row_no_event_and_no_served_candidate_carries_the_fragment() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![MEGA, OTHER]))
        .await
        .expect("batch");

    // Every byte of the database, which covers every row of every table -- `link_candidates`,
    // `events`, `logs` and whatever a later job adds beside them.
    let stored = every_stored_byte(&database, directory.path()).await;
    assert!(
        !holds(&stored, KEY),
        "the key survived somewhere in the database"
    );
    assert!(!holds(&stored, "s3cret"));

    // The JSON a REST or SSE reader is served. The `vault://` reference deliberately has no
    // field on `LinkCandidate`, so widening that struct cannot put one on the wire either.
    for candidate in &candidates {
        let served = serde_json::to_string(candidate).expect("serialize");
        assert!(!served.contains(KEY), "the key reached a reader: {served}");
        assert!(!served.contains("vault://"), "a reference reached a reader");
        assert!(
            !served.contains('#'),
            "a fragment reached a reader: {served}"
        );
    }
    let reread = database
        .get_candidate(candidates[0].id)
        .await
        .expect("read")
        .expect("a candidate");
    let served = serde_json::to_string(&reread).expect("serialize");
    assert!(!served.contains(KEY));
    assert!(!served.contains("vault://"));
}

/// **Ownership moves to the queue.** The enqueue takes the reference onto the download row and
/// clears the candidate's in the same transaction, so deleting the LinkGrabber entry afterwards
/// cannot take the key the queue is about to use — and the key comes back as it went in.
#[tokio::test]
async fn enqueueing_a_candidate_moves_the_secret_onto_the_download() {
    declare_mega();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let (_, _, candidates) = database
        .add_collector_batch(batch(vec![MEGA]))
        .await
        .expect("batch");
    let candidate_id = candidates[0].id;
    let reference = database
        .candidate_secret_fragment_ref(candidate_id)
        .await
        .expect("read")
        .expect("a reference");

    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "mega".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: candidates[0].url.clone(),
            file_name: "10MB.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: Some(Box::new(rd_db::NewSecretFragment {
                reference: reference.clone(),
                candidate_id: Some(candidate_id),
            })),
        })
        .await
        .expect("download");

    // The candidate no longer owns it.
    assert_eq!(
        database
            .candidate_secret_fragment_ref(candidate_id)
            .await
            .expect("read"),
        None
    );
    // The download does, and the fragment comes back byte for byte.
    assert_eq!(
        database
            .download_secret_fragment(download.id)
            .await
            .expect("read"),
        Some(KEY.to_owned())
    );

    // Removing the LinkGrabber entry now must not touch the queue's key.
    database
        .delete_candidate(candidate_id)
        .await
        .expect("delete candidate");
    assert_eq!(
        database
            .download_secret_fragment(download.id)
            .await
            .expect("read"),
        Some(KEY.to_owned())
    );
    assert!(
        !holds(&every_stored_byte(&database, directory.path()).await, KEY),
        "the queue row carries the reference, never the key"
    );

    // And the download taking it away is the last owner there is.
    database
        .delete_download(download.id)
        .await
        .expect("delete download");
    let vault = database.secret_vault().expect("a vault");
    assert!(vault.get_bytes(&reference).await.is_err());
}
