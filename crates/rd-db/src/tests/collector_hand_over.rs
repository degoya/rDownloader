//! RD-1440-07: LinkGrabber links handed to a remote-job provider, settled in the database --
//! marked with their job when they stay, removed with the package they leave empty when not.

use rd_core::IngressSource;

use super::temp_database;
use crate::{Database, HandedOverLinks, NewAccount, NewCollectorBatch};

/// One package of three links, as a paste leaves it.
async fn release(database: &Database) -> Vec<rd_core::LinkCandidate> {
    let urls = [
        "magnet:?xt=urn:btih:c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2&dn=Show.S01",
        "https://files.example/Show.S01E01.mkv",
        "https://files.example/Show.S01E02.mkv",
    ];
    let (_, packages, candidates) = database
        .add_collector_batch(NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: Some("Show.S01".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: urls.iter().map(|url| url.parse().expect("url")).collect(),
            providers: vec![None; urls.len()],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1, "one package for the release");
    candidates
}

/// A job row on one account, the way the remote-job service's duplicate guard writes it.
async fn job(database: &Database, account: rd_core::AccountId, key: &str) -> rd_core::RemoteJobId {
    database
        .claim_remote_job(crate::ClaimRemoteJob {
            id: rd_core::RemoteJobId::new(),
            account_id: account,
            plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
            content_key: key.to_owned(),
            source_kind: rd_core::RemoteJobSourceKind::Address,
            source: key.as_bytes().to_vec(),
            source_name: None,
            package_id: None,
        })
        .await
        .expect("claim")
        .id
}

async fn listed(database: &Database) -> Vec<rd_core::LinkCandidate> {
    database.list_candidates().await.expect("candidates")
}

#[tokio::test]
async fn handed_over_links_are_marked_or_removed_with_the_package_they_empty() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    let account = database
        .create_account(NewAccount {
            provider: "torbox".to_owned(),
            label: "TorBox".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id;
    let candidates = release(&database).await;
    let first = job(&database, account, "first").await;
    let second = job(&database, account, "second").await;

    // Kept: the link stays and names its job and account; the others name nothing.
    let changed = database
        .settle_handed_over_candidates(vec![(candidates[0].id, first)], HandedOverLinks::Mark)
        .await
        .expect("mark");
    assert_eq!(changed, 1);
    let rows = listed(&database).await;
    assert_eq!(rows.len(), 3, "a marked link stays in the LinkGrabber");
    let marked = rows
        .iter()
        .find(|row| row.id == candidates[0].id)
        .expect("the marked link");
    assert_eq!(
        marked.handed_over,
        Some(rd_core::NzbHandOver {
            remote_job_id: first,
            account_id: account,
        })
    );
    assert!(
        rows.iter()
            .filter(|row| row.id != candidates[0].id)
            .all(|row| row.handed_over.is_none())
    );

    // Forgetting the job clears the mark: the link is the person's again.
    assert!(database.delete_remote_job(first).await.expect("forget"));
    assert!(
        listed(&database)
            .await
            .iter()
            .all(|row| row.handed_over.is_none())
    );

    // Removed: the links go, the package stays while one link is left in it.
    let changed = database
        .settle_handed_over_candidates(
            vec![(candidates[0].id, second), (candidates[1].id, second)],
            HandedOverLinks::Remove,
        )
        .await
        .expect("remove");
    assert_eq!(changed, 2);
    let rows = listed(&database).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, candidates[2].id);
    assert_eq!(
        database
            .list_collector_packages()
            .await
            .expect("packages")
            .len(),
        1
    );

    // The last link takes the package with it.
    database
        .settle_handed_over_candidates(vec![(candidates[2].id, second)], HandedOverLinks::Remove)
        .await
        .expect("remove the rest");
    assert!(listed(&database).await.is_empty());
    assert!(
        database
            .list_collector_packages()
            .await
            .expect("packages")
            .is_empty(),
        "a package left without links is deleted"
    );
}
