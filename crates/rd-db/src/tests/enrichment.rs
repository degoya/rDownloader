//! Enrichment carried from indexers and candidates onto packages and downloads.

use chrono::Utc;
use rd_core::{AuthProfileSelection, DownloadId, IngressSource, PackageId};

use crate::{Database, NewDownload, NewPackage};

/// RD-107-02. What the indexer declared reaches the candidate and survives a restart.
///
/// The restart is the point: a poll writes the attributes, the service stops, and the online
/// check that asks an enricher runs afterwards. If they lived anywhere but in the row, the
/// plugin would be asked with nothing.
#[tokio::test]
async fn declared_indexer_attributes_survive_a_reopen_of_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("declared.sqlite");
    let declared: std::collections::BTreeMap<String, String> = [
        ("imdb".to_owned(), "tt0111161".to_owned()),
        ("imdbscore".to_owned(), "9.3".to_owned()),
    ]
    .into_iter()
    .collect();
    let candidate_id = {
        let database = Database::open(path.clone()).await.expect("database");
        let (_, _, candidates) = database
            .add_collector_batch(crate::NewCollectorBatch {
                package_hints: Vec::new(),
                mirror_hints: Vec::new(),
                source: IngressSource::Subscription,
                source_label: Some("Indexer".to_owned()),
                package_name: None,
                password: None,
                passwords: Vec::new(),
                category_id: None,
                priority: None,
                urls: vec![
                    "https://indexer.example/api?t=get&id=1"
                        .parse()
                        .expect("URL"),
                    "https://indexer.example/api?t=get&id=2"
                        .parse()
                        .expect("URL"),
                ],
                providers: vec![None, None],
                file_names: vec![
                    Some("Release.One".to_owned()),
                    Some("Release.Two".to_owned()),
                ],
                sizes: Vec::new(),
                requests: Vec::new(),
                body_refs: Vec::new(),
                auto_check: false,
                // Only the first link was declared; the second stands for every link no
                // subscription produced.
                source_attributes: vec![declared.clone(), std::collections::BTreeMap::new()],
            })
            .await
            .expect("batch");
        assert_eq!(candidates.len(), 2);
        let bare = database
            .candidate_source_attributes(candidates[1].id)
            .await
            .expect("read");
        assert!(bare.is_empty(), "an undeclared link must be asked bare");
        candidates[0].id
    };
    let database = Database::open(path).await.expect("reopen");
    let stored = database
        .candidate_source_attributes(candidate_id)
        .await
        .expect("read");
    assert_eq!(stored, declared);
}

/// RD-107-02. The enricher fields reach the package and its queue rows, and stay there.
#[tokio::test]
async fn enrichment_is_carried_onto_the_package_and_its_downloads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("carry.sqlite");
    let field = |name: &str| rd_core::EnrichmentField {
        name: name.to_owned(),
        value: "9.3".to_owned(),
        plugin_id: "imdb-enricher".to_owned(),
        fetched_at: Utc::now(),
    };
    let (package_id, download_id) = {
        let database = Database::open(path.clone()).await.expect("database");
        let package_id = PackageId::new();
        database
            .create_package(NewPackage {
                id: package_id,
                name: "carry".to_owned(),
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
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id,
                source: "https://example.test/file".parse().expect("URL"),
                file_name: "file".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: rd_core::DownloadState::Queued,
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        // Nothing before the carry: the column is what this test is about.
        assert!(download.enrichment.is_empty());
        database
            .carry_enrichment(
                package_id,
                vec![field("imdb.score")],
                vec![(download.id, vec![field("imdb.score")])],
            )
            .await
            .expect("carry");
        // Twice, because an enqueue can be retried and a replace must say the same thing.
        database
            .carry_enrichment(
                package_id,
                vec![field("imdb.score")],
                vec![(download.id, vec![field("imdb.score")])],
            )
            .await
            .expect("carry again");
        (package_id, download.id)
    };
    let database = Database::open(path).await.expect("reopen");
    let package = database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package");
    assert_eq!(package.enrichment.len(), 1);
    assert_eq!(package.enrichment[0].name, "imdb.score");
    assert_eq!(package.enrichment[0].plugin_id, "imdb-enricher");
    let download = database
        .get_download(download_id)
        .await
        .expect("download read")
        .expect("download");
    assert_eq!(download.enrichment.len(), 1);
    assert_eq!(download.enrichment[0].value, "9.3");
}

/// RD-108-15: the enricher's write and the enqueue are two writer commands with no order
/// between them, so both orders have to end in the same place.
///
/// `when_written` decides which of the two wins the race, and the assertions do not change with
/// it: the fields belong on the package and on the queue row either way. Before this was fixed,
/// everything but [`EnrichedAt::BeforeTheClaim`] was lost — the enqueue copied the snapshot the
/// claim handed it, and the candidate row the fields landed on is detached moments later.
#[derive(Clone, Copy, Debug)]
enum EnrichedAt {
    /// The order that always worked: the fields are there when the promotion claims the links.
    BeforeTheClaim,
    /// The enricher answers while the enqueue is running, before the queue rows exist.
    WhileTheRowsAreWritten,
    /// The enricher answers after the whole promotion is over.
    AfterTheEnqueue,
}

async fn enrichment_survives_the_enqueue(when_written: EnrichedAt) {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("race.sqlite"))
        .await
        .expect("database");
    let url: url::Url = "https://example.test/release.bin".parse().expect("URL");
    let field = rd_core::EnrichmentField {
        name: "imdb.score".to_owned(),
        value: "9.3".to_owned(),
        plugin_id: "imdb-enricher".to_owned(),
        fetched_at: Utc::now(),
    };
    let (_, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Subscription,
            source_label: Some("Indexer".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None],
            urls: vec![url.clone()],
            file_names: vec![None],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let enrich = || database.set_candidate_enrichment(candidates[0].id, vec![field.clone()]);
    if matches!(when_written, EnrichedAt::BeforeTheClaim) {
        enrich().await.expect("enrichment");
    }

    // The promotion, step by step: claim the links, write the queue rows, release the claim.
    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("claim");
    if matches!(when_written, EnrichedAt::WhileTheRowsAreWritten) {
        enrich().await.expect("enrichment");
    }
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "release".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            // What the claim saw, which is what the enqueue has to work from.
            enrichment: claimed[0].0.enrichment.clone(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: url.clone(),
            file_name: "release.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: claimed[0].0.enrichment.clone(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    database
        .finish_package_enqueue(packages[0].id, true, Vec::new())
        .await
        .expect("finish");
    if matches!(when_written, EnrichedAt::AfterTheEnqueue) {
        enrich().await.expect("enrichment");
    }

    let package = database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package");
    assert_eq!(
        package.enrichment.len(),
        1,
        "the package must carry the field, {when_written:?}"
    );
    assert_eq!(package.enrichment[0].name, "imdb.score");
    assert_eq!(package.enrichment[0].plugin_id, "imdb-enricher");
    let stored = database
        .get_download(download.id)
        .await
        .expect("download read")
        .expect("download");
    assert_eq!(
        stored.enrichment.len(),
        1,
        "the queue row must carry it too, {when_written:?}"
    );
    assert_eq!(stored.enrichment[0].value, "9.3");
}

#[tokio::test]
async fn enrichment_written_before_the_claim_reaches_the_package_and_the_download() {
    enrichment_survives_the_enqueue(EnrichedAt::BeforeTheClaim).await;
}

#[tokio::test]
async fn enrichment_written_while_the_enqueue_runs_reaches_the_package_and_the_download() {
    enrichment_survives_the_enqueue(EnrichedAt::WhileTheRowsAreWritten).await;
}

#[tokio::test]
async fn enrichment_written_after_the_enqueue_reaches_the_package_and_the_download() {
    enrichment_survives_the_enqueue(EnrichedAt::AfterTheEnqueue).await;
}
