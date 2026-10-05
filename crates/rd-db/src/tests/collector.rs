//! LinkGrabber intake, grouping, regrouping and the claim for the queue.

use rd_core::IngressSource;

use super::{proposed_pair, routing_category, routing_root};
use crate::{Database, NewCategory, NewCategoryRule, NewStorageRoot};

#[tokio::test]
async fn category_rules_are_applied_when_links_enter_the_collector() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("routing.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let category = database
        .create_category(NewCategory {
            name: "Hoster".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "hoster".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            direct_unpack: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    database
        .create_category_rule(NewCategoryRule {
            name: "DDownload".to_owned(),
            priority: 10,
            source: None,
            domain: Some("ddownload.com".to_owned()),
            protocol: Some("https".to_owned()),
            extension: None,
            mime_type: None,
            name_regex: None,
            category_id: category.id,
            enabled: true,
        })
        .await
        .expect("rule");
    let (_, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Clipboard,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec!["https://ddownload.com/abc123xyz".parse().expect("URL")],
            providers: vec![None],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(candidates[0].category_id, Some(category.id));
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].category_id, Some(category.id));
}

#[tokio::test]
async fn collector_intake_groups_multipart_links_and_locks_packages_for_enqueue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector.sqlite"))
        .await
        .expect("database");
    // Archive passwords live in the vault (RD-190-04).
    database
        .install_file_vault(directory.path().join("secrets"))
        .await
        .expect("vault");
    let urls: Vec<url::Url> = [
        "https://ddownload.com/aaa111bbb/Game.part1.rar",
        "https://ddownload.com/aaa222bbb/Game.part2.rar",
        "https://1fichier.com/?xyz",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: Some("pw".to_owned()),
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 2, "archive set + loose link");
    assert_eq!(packages[0].name, "Game");
    assert!(packages[0].has_password);
    assert!(
        candidates
            .iter()
            .all(|c| c.state == rd_core::LinkCandidateState::Checking)
    );
    assert_eq!(candidates[0].position, 1);
    assert_eq!(candidates[1].position, 2);

    // A checking package cannot be enqueued; recording results unlocks it.
    assert!(
        database
            .claim_package_for_enqueue(packages[0].id, None)
            .await
            .is_err()
    );
    for candidate in &candidates {
        database
            .record_candidate_check(
                candidate.id,
                Some(rd_core::LinkCheckResult {
                    url: candidate.url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some("Renamed.part1.rar".to_owned()),
                    size: rd_core::ByteCount::new(10).ok(),
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("record");
    }
    let listed = database.list_candidates().await.expect("candidates");
    assert!(
        listed
            .iter()
            .all(|c| c.state == rd_core::LinkCandidateState::Online)
    );
    assert_eq!(listed[0].size.map(|s| s.get()), Some(10));

    // Reorder packages: loose link first.
    database
        .reorder_collector_packages(vec![packages[1].id, packages[0].id])
        .await
        .expect("reorder");
    let ordered = database.list_collector_packages().await.expect("packages");
    assert_eq!(ordered[0].id, packages[1].id);

    // Claim locks all links atomically; failure restores the previous states.
    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 2);
    assert!(
        database
            .claim_package_for_enqueue(packages[0].id, None)
            .await
            .is_err()
    );
    let restore: Vec<_> = claimed
        .iter()
        .map(|(c, previous)| (c.id, *previous))
        .collect();
    database
        .finish_package_enqueue(packages[0].id, false, restore)
        .await
        .expect("restore");
    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("claim again");
    database
        .finish_package_enqueue(packages[0].id, true, Vec::new())
        .await
        .expect("finish");
    assert_eq!(claimed.len(), 2);
    let remaining = database.list_collector_packages().await.expect("packages");
    assert_eq!(remaining.len(), 1, "enqueued package disappears");
    assert_eq!(remaining[0].batch_id, batch.id);
}

#[tokio::test]
async fn collector_groups_keep_the_password_of_their_own_declared_link() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-passwords.sqlite"))
        .await
        .expect("database");
    // Archive passwords live in the vault (RD-190-04).
    database
        .install_file_vault(directory.path().join("secrets"))
        .await
        .expect("vault");
    let urls = vec![
        "https://indexer.test/get/one.nzb".parse().expect("url"),
        "https://indexer.test/get/two.nzb".parse().expect("url"),
        "https://indexer.test/get/three.nzb".parse().expect("url"),
    ];
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Subscription,
            source_label: Some("Indexer".to_owned()),
            package_name: None,
            password: None,
            passwords: vec![
                Some("first-secret".to_owned()),
                Some("second-secret".to_owned()),
                None,
            ],
            category_id: None,
            priority: None,
            providers: vec![Some(rd_core::NZB_PROVIDER.to_owned()); 3],
            urls,
            file_names: vec![
                Some("First release".to_owned()),
                Some("Second release".to_owned()),
                Some("No password release".to_owned()),
            ],
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    assert_eq!(packages.len(), 3);
    let password = |name: &str| {
        packages
            .iter()
            .find(|package| package.name == name)
            .and_then(|package| package.password.as_deref())
    };
    assert_eq!(password("First release"), Some("first-secret"));
    assert_eq!(password("Second release"), Some("second-secret"));
    assert_eq!(password("No password release"), None);
}

/// One statement per changed column, one re-read, and the order the caller asked for.
///
/// The per-id loop this replaced issued up to eight statements plus a read for every package,
/// which is what made a bulk edit of a few hundred packages block the single writer.
#[tokio::test]
async fn updating_many_collector_packages_returns_them_in_the_requested_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-bulk.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let category = routing_category(&database, root, "Movies", true).await;
    let urls: Vec<url::Url> = [
        "https://ddownload.com/aaa111bbb/Game.part1.rar",
        "https://ddownload.com/aaa222bbb/Game.part2.rar",
        "https://1fichier.com/?xyz",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 2, "archive set + loose link");

    // Reversed, plus an id nobody has: the reply must follow the request and skip the stranger.
    let updated = database
        .update_collector_packages(
            vec![
                packages[1].id,
                packages[0].id,
                rd_core::CollectorPackageId::new(),
            ],
            crate::CollectorPackageChange {
                category_id: Some(Some(category.id)),
                priority: Some(rd_core::DownloadPriority::High),
                ..crate::CollectorPackageChange::default()
            },
        )
        .await
        .expect("update");

    assert_eq!(
        updated.iter().map(|package| package.id).collect::<Vec<_>>(),
        vec![packages[1].id, packages[0].id]
    );
    assert!(
        updated
            .iter()
            .all(|package| package.category_id == Some(category.id)
                && package.priority == rd_core::DownloadPriority::High)
    );
    // The candidates carry the same routing, which is what the second statement per column does.
    let candidates = database.list_candidates().await.expect("candidates");
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.category_id == Some(category.id)
                && candidate.priority == rd_core::DownloadPriority::High)
    );
}

/// Regrouping reads and groups outside the write transaction and still regroups.
///
/// The names the online check revealed split one auto-named package into two; the write phase
/// has to create the missing package, move the candidates and drop what is left empty.
#[tokio::test]
async fn regrouping_splits_a_batch_once_the_check_revealed_the_real_names() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-regroup.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://ddownload.com/aaa111bbb/one",
        "https://ddownload.com/aaa222bbb/two",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(
        packages.len(),
        1,
        "two nameless links share one auto-named package"
    );
    for (candidate, name) in candidates
        .iter()
        .zip(["Movie.part1.rar", "Series.S01E01.mkv"])
    {
        database
            .record_candidate_check(
                candidate.id,
                Some(rd_core::LinkCheckResult {
                    url: candidate.url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some(name.to_owned()),
                    size: None,
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("record");
    }

    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let regrouped = database.list_collector_packages().await.expect("packages");
    assert_eq!(regrouped.len(), 2, "one release per revealed name");
    assert!(
        regrouped.iter().any(|package| package.name == "Movie"),
        "the archive set is named after its base name, got {:?}",
        regrouped
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>()
    );
    let candidates = database.list_candidates().await.expect("candidates");
    let package_ids: std::collections::BTreeSet<_> = candidates
        .iter()
        .filter_map(|candidate| candidate.package_id)
        .collect();
    assert_eq!(package_ids.len(), 2, "the two links no longer share one");
}

/// The downmagaz case of RD-120-17, in the order it actually happens.
///
/// A site rule reads the release title off the page and hands it to every link it found as
/// `package_hint`, so intake names the package after it. The online check then runs, one of
/// the links sits on a service with no resolver and therefore never gets a file name, and the
/// check ends -- always -- in a regroup of the batch.
///
/// That regroup was where the name went. It re-derived every auto-named package from the file
/// names then known, with no hint to go on, so the stated title was replaced: by the common
/// stem when enough names had arrived, and otherwise by the host of the first link without
/// one -- which is why it looked as though the unsupported hoster had renamed the package.
/// It had not; it had only supplied the fallback. A package the source named is no longer
/// auto-named, so the regroup does not reach it.
///
/// The two addresses are the ones RD-120-19 measured on the reported page
/// (`docs/adr/0019-the-address-a-board-shows-is-not-the-hoster.md`): the page carries exactly
/// these two and no others. Neither is a hoster -- both are affiliate link-cloakers -- so the
/// reporter's suspicion that an unsupported hoster renamed the package is wrong twice over.
/// `nfile.cc` hides `novafile.org`, which nothing claims, so the check answers `unsupported`.
/// `dwp.la` hides `downup.me`, which `plugins/xfs-generic` already claims; its file name is
/// what the check returns for it once the redirect is followed, which is RD-120-19's subject.
/// Until it is, that link has no name either, and the regroup reaches the same fallback one
/// step earlier -- a single loose name yields no common stem, so both orders end at
/// `nfile.cc`, the host of the first loose link, which is the name the owner saw.
#[tokio::test]
async fn a_package_name_a_site_rule_stated_survives_the_regroup_after_the_check() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-hint.sqlite"))
        .await
        .expect("database");
    const TITLE: &str = "The Economist USA 09.19.2026";
    let urls: Vec<url::Url> = [
        // The link the reporter suspected. Nothing claims what it hides, so the check ends in
        // `unsupported` and this candidate never gets a file name -- and, being first, it is
        // the host the fallback would have used.
        "https://nfile.cc/qK7XDAwq",
        "https://dwp.la/d/dro",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            // What `rd_plugin_ext::FolderCrawlers` puts on every link a rule produced.
            package_hints: vec![Some(TITLE.to_owned()); urls.len()],
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            // No explicit name: the rule's title is the only one there is, which is exactly
            // the case that used to be treated as a guess.
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1, "one page, one package");
    assert_eq!(packages[0].name, TITLE, "intake takes the rule's title");
    assert!(
        !packages[0].auto_named,
        "a name the source stated is not a guess"
    );

    for (candidate, name) in candidates.iter().zip([
        None,
        Some("The_Economist_USA_-_19_September_2026_downmagaz.net.pdf"),
    ]) {
        match name {
            Some(name) => database
                .record_candidate_check(
                    candidate.id,
                    Some(rd_core::LinkCheckResult {
                        url: candidate.url.clone(),
                        status: rd_core::LinkStatus::Online,
                        file_name: Some(name.to_owned()),
                        size: None,
                        media: None,
                    }),
                    None,
                    false,
                    None,
                )
                .await
                .expect("record"),
            None => database
                .mark_candidate_unsupported(
                    candidate.id,
                    rd_core::CandidateMessage::coded(
                        "collector.check_unknown",
                        "no service can check this address",
                    ),
                    None,
                )
                .await
                .expect("unsupported"),
        }
    }

    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let after = database.list_collector_packages().await.expect("packages");
    assert_eq!(
        after.len(),
        1,
        "the batch is still one package, got {after:?}"
    );
    assert_eq!(
        after[0].name, TITLE,
        "the regroup must not rename what the rule stated"
    );
    let candidates = database.list_candidates().await.expect("candidates");
    assert_eq!(candidates.len(), 2);
    for candidate in &candidates {
        assert_eq!(
            candidate.package_id,
            Some(after[0].id),
            "every link stays in the package the rule named"
        );
    }
}

/// The getcomics case of RD-120-17: the same loss, with not one file name to fall back on.
///
/// The title and the addresses are the ones the shipped rule reads out of the recorded page
/// `crates/rd-siterules/tests/fixtures/getcomics-release.html`; that the rule produces exactly
/// these is what `rd-siterules`' `a_getcomics_post_becomes_a_package_of_hoster_links` asserts,
/// and this case takes them from there and carries them the rest of the way.
///
/// The rule yields six addresses out of that page; the three used here are its file hosts,
/// in the order the fixture carries them. The check named none of them -- which is the case
/// the owner saw -- so the regroup had no common stem either and fell straight through to the
/// host of the first link: the comic's title became `1024terabox.com`. From the outside that
/// reads as "the name did not come through at all", which is how the owner reported it, but
/// it is the same regroup as the downmagaz case. Had the check read a name out of the
/// `datanodes.to` path instead, the package would have been renamed to that file's stem --
/// still not the comic's title, and still the same line.
#[tokio::test]
async fn a_getcomics_package_keeps_the_title_when_no_link_reveals_a_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-getcomics.sqlite"))
        .await
        .expect("database");
    const TITLE: &str = "Absolute Green Arrow #5 (2026)";
    let urls: Vec<url::Url> = [
        "https://1024terabox.com/s/13Fmfzo7LL0FMZ8j1l4ANGw",
        "https://vikingfile.com/f/7hJu75ACQL",
        "https://datanodes.to/6ass8jkpo3j9/Absolute_Green_Arrow_005_(2026)_(digital)_(Pyrate-DCP).cbz",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: vec![Some(TITLE.to_owned()); urls.len()],
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, TITLE);

    // The check answers for all three and names none of them, which is what these hosters do.
    for candidate in &candidates {
        database
            .record_candidate_check(
                candidate.id,
                Some(rd_core::LinkCheckResult {
                    url: candidate.url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: None,
                    size: None,
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("record");
    }
    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let after = database.list_collector_packages().await.expect("packages");
    assert_eq!(after.len(), 1, "got {after:?}");
    assert_eq!(
        after[0].name, TITLE,
        "the comic's title, not the first address's host name"
    );
}

/// A link whose check failed outright stays queueable.
///
/// Reported from use: a DDownload account whose sign-in did not work made the batched check
/// fail, every link of the batch landed in `error`, and the package could then not be added to
/// the downloader at all. `error` was the only state `claim_package_for_enqueue` left out —
/// stricter than `offline`, which means the file is known to be gone.
#[tokio::test]
async fn a_package_whose_check_failed_can_still_be_enqueued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("failed-check.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = ["https://ddownload.com/aaa111bbb/Movie.mkv"]
        .iter()
        .map(|value| value.parse().expect("URL"))
        .collect();
    let (_batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    // No result at all — what the account failure produces.
    database
        .record_candidate_check(
            candidates[0].id,
            None,
            Some(rd_core::CandidateMessage::plain(
                "Provider account check failed",
            )),
            false,
            None,
        )
        .await
        .expect("record");
    let listed = database.list_candidates().await.expect("candidates");
    assert_eq!(listed[0].state, rd_core::LinkCandidateState::Error);
    assert!(
        listed[0]
            .error
            .as_deref()
            .is_some_and(|message| message.contains("account check failed")),
        "the reason has to survive, or nobody can tell why nothing was confirmed"
    );

    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("a link that could not be checked is still the user's call");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].1, rd_core::LinkCandidateState::Error);
}

/// A claim narrowed to some links of a package leaves the others where they were.
///
/// Reported from use: with the LinkGrabber filtered to one hoster, "add to the queue" sent the
/// links of every other hoster along. The links the claim leaves out keep their state and their
/// package, so the package outlives the enqueue with exactly them in it.
#[tokio::test]
async fn a_narrowed_claim_leaves_the_other_links_in_their_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("narrowed-claim.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://one.example/a.bin",
        "https://two.example/b.bin",
        "https://one.example/c.bin",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (_batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1);
    let hidden = candidates[1].id;

    let claimed = database
        .claim_package_for_enqueue(
            packages[0].id,
            Some(vec![candidates[0].id, candidates[2].id]),
        )
        .await
        .expect("claim");
    let claimed_ids: Vec<_> = claimed.iter().map(|(candidate, _)| candidate.id).collect();
    assert_eq!(claimed_ids, [candidates[0].id, candidates[2].id]);
    database
        .finish_package_enqueue(packages[0].id, true, Vec::new())
        .await
        .expect("finish");

    let listed = database.list_candidates().await.expect("candidates");
    let left = listed
        .iter()
        .find(|candidate| candidate.id == hidden)
        .expect("the hidden link is still in the LinkGrabber");
    assert_eq!(left.package_id, Some(packages[0].id));
    assert_eq!(left.state, candidates[1].state);
    let remaining = database.list_collector_packages().await.expect("packages");
    assert_eq!(
        remaining.len(),
        1,
        "the package stays for the link it still holds"
    );
    assert_eq!(remaining[0].id, packages[0].id);

    // A narrowing that names nothing enqueueable in the package is refused like an empty one.
    let error = database
        .claim_package_for_enqueue(packages[0].id, Some(vec![candidates[0].id]))
        .await
        .expect_err("nothing of this package is named");
    assert_eq!(
        crate::store_kind(&error),
        Some(crate::StoreErrorKind::NoEnqueueableLinks)
    );
}

/// Links from three batches moved into a new package stay in it when the first batch's own
/// links are deleted. The package took the first link's batch, and deleting that batch's last
/// link used to delete the batch and — by its cascade — the package, which left the other
/// four links with no package and invisible in the LinkGrabber.
#[tokio::test]
async fn a_package_built_by_a_move_outlives_the_batch_it_was_named_after() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("move-orphan.sqlite"))
        .await
        .expect("database");
    let first = proposed_pair(&database, "Batch.One").await;
    let second = proposed_pair(&database, "Batch.Two").await;
    let third = proposed_pair(&database, "Batch.Three").await;
    let ids: Vec<rd_core::CandidateId> = first
        .iter()
        .chain(&second)
        .chain(&third)
        .map(|candidate| candidate.id)
        .collect();
    let package = database
        .move_candidates(
            ids.clone(),
            crate::MoveTarget::New {
                name: "Together".to_owned(),
            },
        )
        .await
        .expect("move");

    for candidate in &first {
        database
            .delete_candidate(candidate.id)
            .await
            .expect("delete");
    }

    let left = database.list_candidates().await.expect("candidates");
    assert_eq!(left.len(), 4);
    assert!(
        left.iter()
            .all(|candidate| candidate.package_id == Some(package.id)),
        "every remaining link is still in the package: {left:?}"
    );
    assert!(
        database
            .list_collector_packages()
            .await
            .expect("packages")
            .iter()
            .any(|kept| kept.id == package.id),
        "the package outlives the batch it was named after"
    );
}
