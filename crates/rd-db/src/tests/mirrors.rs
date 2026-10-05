//! Mirror groups: declared, proposed, preferred, pinned and dissolved.

use rd_core::{AuthProfileSelection, DownloadId, IngressSource, PackageId};

use super::proposed_pair;
use crate::{Database, NewDownload, NewPackage};

/// A LinkGrabber batch with one mirror hint per link, everything else left at nothing.
pub(super) fn mirror_batch(
    urls: &[&str],
    file_names: Vec<Option<String>>,
    hints: Vec<Option<rd_core::MirrorHint>>,
) -> crate::NewCollectorBatch {
    crate::NewCollectorBatch {
        package_hints: Vec::new(),
        mirror_hints: hints,
        source: IngressSource::Manual,
        source_label: None,
        package_name: Some("Release".to_owned()),
        password: None,
        passwords: Vec::new(),
        category_id: None,
        priority: None,
        urls: urls.iter().map(|url| url.parse().expect("URL")).collect(),
        providers: vec![None; urls.len()],
        file_names,
        sizes: Vec::new(),
        requests: Vec::new(),
        body_refs: Vec::new(),
        auto_check: false,
        source_attributes: Vec::new(),
    }
}

/// Source 1, and the criterion that the group outlives the process (RD-110-18).
///
/// A release page states that its five links are the same file. Nothing about the links
/// themselves says so — five hosters, five names — so this is the only source that can group
/// them, and it has to reach the database rather than a value someone computed once.
#[tokio::test]
async fn a_declared_mirror_group_survives_a_reopen_of_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("mirrors.sqlite");
    let hint = rd_core::MirrorHint {
        group: "release-page|https://board.example.org/a".to_owned(),
        quality: Some("1080p".to_owned()),
        language: Some("German".to_owned()),
    };
    let urls = [
        "https://one.example/a",
        "https://two.example/b",
        "https://three.example/c",
        "https://four.example/d",
        "https://five.example/e",
    ];
    {
        let database = Database::open(path.clone()).await.expect("database");
        let (_, _, candidates) = database
            .add_collector_batch(mirror_batch(
                &urls,
                vec![
                    Some("one.rar".to_owned()),
                    Some("two.rar".to_owned()),
                    Some("three.rar".to_owned()),
                    Some("four.rar".to_owned()),
                    Some("five.rar".to_owned()),
                ],
                vec![Some(hint.clone()); urls.len()],
            ))
            .await
            .expect("batch");
        assert_eq!(candidates.len(), 5);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.mirror.is_some()),
            "all five links the page declared are one group"
        );
    }
    // Reopening proves the group lives in the database, not in the value the intake returned.
    let database = Database::open(path).await.expect("reopen");
    let candidates = database.list_candidates().await.expect("candidates");
    assert_eq!(candidates.len(), 5);
    let mirrors: Vec<&rd_core::CandidateMirror> = candidates
        .iter()
        .filter_map(|c| c.mirror.as_ref())
        .collect();
    assert_eq!(mirrors.len(), 5);
    assert!(
        mirrors
            .iter()
            .all(|mirror| mirror.group == mirrors[0].group),
        "one group, not five"
    );
    assert!(
        mirrors
            .iter()
            .all(|mirror| mirror.source == rd_core::MirrorSource::Declared)
    );
    assert_eq!(
        mirrors.iter().filter(|mirror| mirror.selected).count(),
        1,
        "exactly one mirror is the chosen one"
    );
    assert_eq!(mirrors[0].quality.as_deref(), Some("1080p"));
    assert_eq!(mirrors[0].language.as_deref(), Some("German"));
}

/// An address counts as a duplicate while it is still in the LinkGrabber or in the download
/// list; once its download is deleted, taking it in again is a fresh intake, not a duplicate.
#[tokio::test]
async fn a_deleted_download_no_longer_makes_its_address_a_duplicate() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("duplicate-after-delete.sqlite");
    let database = Database::open(&path).await.expect("database");
    let url = "https://host.example/file.rar";
    let intake = || mirror_batch(&[url], vec![Some("file.rar".to_owned())], vec![None]);
    let (_, _, first) = database.add_collector_batch(intake()).await.expect("first");
    assert_ne!(first[0].state, rd_core::LinkCandidateState::Duplicate);

    // Queued: the candidate stays behind as `enqueued`, the download exists.
    let mut connection = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .expect("connect");
    sqlx::query("UPDATE link_candidates SET state = 'enqueued', package_id = NULL WHERE url = ?")
        .bind(url)
        .execute(&mut connection)
        .await
        .expect("enqueued");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "file".to_owned(),
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
            source: url.parse().expect("URL"),
            file_name: "file.rar".to_owned(),
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
    let (_, _, while_queued) = database.add_collector_batch(intake()).await.expect("again");
    assert_eq!(
        while_queued[0].state,
        rd_core::LinkCandidateState::Duplicate,
        "still in the download list"
    );

    // Deleted from the queue: the address is new again.
    database.delete_download(download.id).await.expect("delete");
    sqlx::query("DELETE FROM link_candidates WHERE state = 'duplicate'")
        .execute(&mut connection)
        .await
        .expect("clear the second intake");
    let (_, _, after_delete) = database.add_collector_batch(intake()).await.expect("after");
    assert_ne!(
        after_delete[0].state,
        rd_core::LinkCandidateState::Duplicate,
        "an enqueued row whose download is gone is no duplicate"
    );
}

/// A mirror is not a duplicate, and the duplicate state must not swallow one (RD-110-18).
///
/// Five different addresses for one file are five mirrors; the same address a second time is
/// a duplicate and is no mirror of the first, because starting it would fetch the very bytes
/// the first one already failed to get.
#[tokio::test]
async fn a_mirror_is_not_marked_as_a_duplicate() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-duplicate.sqlite"))
        .await
        .expect("database");
    let hint = rd_core::MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let urls = [
        "https://one.example/a",
        "https://two.example/b",
        "https://three.example/c",
        "https://four.example/d",
        "https://five.example/e",
    ];
    let (_, _, candidates) = database
        .add_collector_batch(mirror_batch(
            &urls,
            vec![Some("release.rar".to_owned()); urls.len()],
            vec![Some(hint.clone()); urls.len()],
        ))
        .await
        .expect("batch");
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.state != rd_core::LinkCandidateState::Duplicate),
        "a second hoster is not a second copy of the link"
    );
    assert_eq!(
        candidates
            .iter()
            .filter(|candidate| candidate.mirror.is_some())
            .count(),
        5
    );
    // The same address again: that one *is* a duplicate, and it joins no group.
    let (_, _, again) = database
        .add_collector_batch(mirror_batch(
            &urls[..1],
            vec![Some("release.rar".to_owned())],
            vec![Some(hint)],
        ))
        .await
        .expect("second batch");
    assert_eq!(again[0].state, rd_core::LinkCandidateState::Duplicate);
    assert!(again[0].mirror.is_none(), "a duplicate mirrors nothing");
    // And the five it was a copy of kept their group.
    let listed = database.list_candidates().await.expect("candidates");
    assert_eq!(
        listed
            .iter()
            .filter(|candidate| candidate.mirror.is_some())
            .count(),
        5
    );
}

/// Sources 2 and 3: what the online check leaves behind becomes the group (RD-110-18).
///
/// At intake these two links have no name and no size, so nothing can be said about them.
/// The check fills both in, and the regroup that follows it is where the answer changes from
/// "no idea" to "the same file" — which is why the grouping is recomputed there and not only
/// once at intake.
#[tokio::test]
async fn the_online_check_turns_names_and_sizes_into_a_mirror_group() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-check.sqlite"))
        .await
        .expect("database");
    let urls = ["https://one.example/dl", "https://two.example/dl"];
    let (batch, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            // No package name: only an automatically named package is regrouped, which is
            // exactly the one the online check is allowed to rearrange.
            package_name: None,
            ..mirror_batch(&urls, Vec::new(), Vec::new())
        })
        .await
        .expect("batch");
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.mirror.is_none()),
        "nothing is known about these links yet"
    );
    let ids: Vec<rd_core::CandidateId> = candidates.iter().map(|candidate| candidate.id).collect();
    database
        .claim_candidates_for_check(ids.clone())
        .await
        .expect("claim");
    for (index, id) in ids.iter().enumerate() {
        database
            .record_candidate_check(
                *id,
                Some(rd_core::LinkCheckResult {
                    url: urls[index].parse().expect("URL"),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some("Show.S01E01.German.1080p.mkv".to_owned()),
                    size: rd_core::ByteCount::new(1_000_000).ok(),
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("check");
    }
    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");
    let listed = database.list_candidates().await.expect("candidates");
    let mirrors: Vec<&rd_core::CandidateMirror> =
        listed.iter().filter_map(|c| c.mirror.as_ref()).collect();
    assert_eq!(mirrors.len(), 2, "the check made these two one file");
    assert_eq!(mirrors[0].group, mirrors[1].group);
    assert_eq!(mirrors[0].source, rd_core::MirrorSource::NameAndSize);
    assert_eq!(mirrors.iter().filter(|mirror| mirror.selected).count(), 1);
    // The facets the release name spells out, for the selection that comes next (RD-110-19).
    assert_eq!(mirrors[0].quality.as_deref(), Some("1080p"));
    assert_eq!(mirrors[0].language.as_deref(), Some("German"));
}

/// A name alone is a proposal, and it has to read as one: the two links below never
/// reported a size, so nothing corroborated the name they share.
#[tokio::test]
async fn a_shared_name_without_a_size_is_only_a_proposed_mirror() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-proposal.sqlite"))
        .await
        .expect("database");
    let urls = ["https://one.example/a", "https://two.example/b"];
    let (_, _, candidates) = database
        .add_collector_batch(mirror_batch(
            &urls,
            vec![Some("Show.S01E01.mkv".to_owned()); 2],
            Vec::new(),
        ))
        .await
        .expect("batch");
    let mirrors: Vec<&rd_core::CandidateMirror> = candidates
        .iter()
        .filter_map(|candidate| candidate.mirror.as_ref())
        .collect();
    assert_eq!(mirrors.len(), 2);
    assert_eq!(mirrors[0].source, rd_core::MirrorSource::Name);
}

/// One release page's links, as a site rule delivers them: a declared group, three qualities.
///
/// The names differ, so only the declaration can hold them together — which is the case the
/// preference is interesting in, because it then has a genuine choice to make.
async fn release_page(
    database: &Database,
    prefix: &str,
    names: &[&str],
) -> Vec<rd_core::LinkCandidate> {
    let urls: Vec<url::Url> = names
        .iter()
        .enumerate()
        .map(|(index, _)| {
            format!("https://h{index}.example/{prefix}/{index}")
                .parse()
                .expect("URL")
        })
        .collect();
    let hint = rd_core::MirrorHint {
        group: prefix.to_owned(),
        quality: None,
        language: None,
    };
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: vec![Some(prefix.to_owned()); urls.len()],
            mirror_hints: vec![Some(hint); urls.len()],
            source: IngressSource::Manual,
            source_label: None,
            package_name: Some(prefix.to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: names.iter().map(|name| Some((*name).to_owned())).collect(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    candidates
}

/// The chosen mirror's file name, read back from the store rather than from the return value.
async fn chosen_mirror(database: &Database, group: &str) -> String {
    let candidates = database.list_candidates().await.expect("candidates");
    candidates
        .into_iter()
        .find(|candidate| {
            candidate
                .mirror
                .as_ref()
                .is_some_and(|mirror| mirror.group == group && mirror.selected)
        })
        .and_then(|candidate| candidate.file_name)
        .unwrap_or_default()
}

/// The standing preference survives the package it was set in: it decides the mirror of a
/// package that arrives afterwards, without being set again (RD-110-19).
#[tokio::test]
async fn a_mirror_preference_decides_the_next_package_too() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-preference.sqlite"))
        .await
        .expect("database");
    release_page(
        &database,
        "first",
        &["Show.E01.German.720p.mkv", "Show.E01.German.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "first").await,
        "Show.E01.German.720p.mkv",
        "with nothing preferred the first member stays chosen"
    );

    database
        .set_mirror_preference(rd_core::MirrorPreference {
            quality: Some("1080p".to_owned()),
            language: None,
            hoster: None,
            ..rd_core::MirrorPreference::default()
        })
        .await
        .expect("preference");
    assert_eq!(
        chosen_mirror(&database, "first").await,
        "Show.E01.German.1080p.mkv",
        "the preference re-chooses the groups that already exist"
    );

    // The package that arrives afterwards never saw the preference being set.
    release_page(
        &database,
        "second",
        &["Other.E02.German.720p.mkv", "Other.E02.German.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "second").await,
        "Other.E02.German.1080p.mkv",
        "a preference that stops at the package it was set in is no preference"
    );
    assert_eq!(
        database.mirror_preference().await.expect("read").quality,
        Some("1080p".to_owned())
    );
}

/// Hidden hosters are stored with the preference and outlive a restart (RD-130-21), and a group
/// whose first member sits at a hidden hoster chooses a shown one instead — before and after the
/// restart, and for a package that arrives afterwards.
#[tokio::test]
async fn hidden_hosters_survive_a_restart_and_are_never_the_chosen_mirror() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("hidden-hosters.sqlite");
    {
        let database = Database::open(path.clone()).await.expect("database");
        release_page(
            &database,
            "first",
            &["Show.E01.720p.mkv", "Show.E01.1080p.mkv"],
        )
        .await;
        assert_eq!(chosen_mirror(&database, "first").await, "Show.E01.720p.mkv");
        database
            .set_mirror_preference(rd_core::MirrorPreference {
                hidden_hosters: vec!["h0.example".to_owned(), "gone.example".to_owned()],
                ..rd_core::MirrorPreference::default()
            })
            .await
            .expect("preference");
        assert_eq!(
            chosen_mirror(&database, "first").await,
            "Show.E01.1080p.mkv",
            "the member at the hidden hoster stops being the chosen one"
        );
    }
    let database = Database::open(path).await.expect("reopen");
    assert_eq!(
        database
            .mirror_preference()
            .await
            .expect("read")
            .hidden_hosters,
        ["h0.example", "gone.example"],
        "a hoster hidden before the restart is still hidden after it"
    );
    assert_eq!(
        chosen_mirror(&database, "first").await,
        "Show.E01.1080p.mkv"
    );
    release_page(
        &database,
        "second",
        &["Other.E02.720p.mkv", "Other.E02.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "second").await,
        "Other.E02.1080p.mkv",
        "the package that arrives afterwards is chosen under the same hidden hosters"
    );
}

/// A mirror somebody pinned is the package's way out of the standing preference, and it stays
/// that way: neither a later preference nor a regroup takes the decision back.
#[tokio::test]
async fn a_pinned_mirror_overrides_the_preference_and_survives_a_regroup() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-pin.sqlite"))
        .await
        .expect("database");
    database
        .set_mirror_preference(rd_core::MirrorPreference {
            quality: Some("1080p".to_owned()),
            language: None,
            hoster: None,
            ..rd_core::MirrorPreference::default()
        })
        .await
        .expect("preference");
    let candidates = release_page(
        &database,
        "release",
        &["Show.E01.720p.mkv", "Show.E01.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.1080p.mkv"
    );

    let pinned = database
        .set_mirror_pin(candidates[0].id, true)
        .await
        .expect("pin");
    assert!(pinned, "the link is in a group, so there was a choice");
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.720p.mkv",
        "the pin outranks the preference"
    );

    // A second preference, and a regroup on top of it: both rewrite `mirror_selected` in full.
    database
        .set_mirror_preference(rd_core::MirrorPreference {
            quality: Some("1080p".to_owned()),
            language: None,
            hoster: None,
            ..rd_core::MirrorPreference::default()
        })
        .await
        .expect("preference again");
    let batch = candidates[0].batch_id;
    database
        .regroup_collector_batches(vec![batch])
        .await
        .expect("regroup");
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.720p.mkv",
        "a regroup must not quietly revise a decision somebody made"
    );

    // Releasing it hands the group back to the preference.
    assert!(
        database
            .set_mirror_pin(candidates[0].id, false)
            .await
            .expect("release")
    );
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.1080p.mkv"
    );
}

/// How many of these candidates are in a mirror group, read back from the store.
async fn grouped_count(database: &Database, ids: &[rd_core::CandidateId]) -> usize {
    database
        .list_candidates()
        .await
        .expect("candidates")
        .into_iter()
        .filter(|candidate| ids.contains(&candidate.id) && candidate.mirror.is_some())
        .count()
}

/// RD-110-34, and the criterion the whole job lives on: a dissolved proposal stays dissolved
/// through **all three** places that recompute the groups.
///
/// The order below is the order the three would undo it. The online check is the dangerous
/// one — it hands the same two links a name *and* a matching size, which is the evidence that
/// would promote the refused proposal to a name-and-size group — so the refusal is stored
/// about the pair rather than about the source that happened to produce it. Intake runs next,
/// and has to leave the decision alone while still grouping links it says nothing about. The
/// move is last, and carries the decision into another package, where the two meet two more
/// links of the same name: the set they would join is the one that was refused, so none of
/// the four is grouped.
#[tokio::test]
async fn a_dissolved_proposal_survives_the_check_an_intake_and_a_move() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-dissolve.sqlite"))
        .await
        .expect("database");
    let first = proposed_pair(&database, "Show.S01E01").await;
    let ids: Vec<rd_core::CandidateId> = first.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        first[0].mirror.as_ref().expect("a group").source,
        rd_core::MirrorSource::Name,
        "a shared name and nothing else is a proposal"
    );

    assert_eq!(
        database
            .dissolve_mirror_group(ids[0])
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::Dissolved
    );
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "the proposal is gone and its links stand on their own"
    );

    // Path one: the online check, which fills in the very evidence that would have promoted
    // the group to `name_and_size`.
    database
        .claim_candidates_for_check(ids.clone())
        .await
        .expect("claim");
    for (index, id) in ids.iter().enumerate() {
        database
            .record_candidate_check(
                *id,
                Some(rd_core::LinkCheckResult {
                    url: first[index].url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some("Show.S01E01.mkv".to_owned()),
                    size: rd_core::ByteCount::new(1_000_000).ok(),
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("check");
    }
    database
        .regroup_collector_batches(vec![first[0].batch_id])
        .await
        .expect("regroup");
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "a size arriving afterwards does not overrule the person who looked at both files"
    );

    // Path two: intake. It recomputes the packages of the batch it wrote, and it must group
    // the links the refusal never named while leaving the refused pair alone.
    let second = proposed_pair(&database, "Other.S01E01").await;
    let other: Vec<rd_core::CandidateId> = second.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        grouped_count(&database, &other).await,
        2,
        "links a refusal never named are grouped as before"
    );
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "and the refused pair is untouched by an intake elsewhere"
    );

    // Path three: the move. Both refused links go into the package of two further links that
    // share their file name, which is the set the refusal poisoned.
    let third = proposed_pair(&database, "Show.S01E01b").await;
    let target = third[0].package_id.expect("a package");
    for id in &ids {
        database
            .set_candidate_file_name(*id, "Show.S01E01b.mkv".to_owned())
            .await
            .expect("rename");
    }
    database
        .move_candidates(ids.clone(), crate::MoveTarget::Existing(target))
        .await
        .expect("move");
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "the decision travels with the links into another package"
    );
    let joined: Vec<rd_core::CandidateId> = third.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        grouped_count(&database, &joined).await,
        0,
        "and the set the two would have joined is the one that was refused"
    );
}

/// A declaration and a name-and-size agreement are refused rather than asked about
/// (RD-110-34).
///
/// A contradiction against either is a finding about the *source* — a site rule that declares
/// wrongly, two files that genuinely agree on name and size — and taking one apart would fix
/// a single package while leaving the rule to do the same thing on the next page.
#[tokio::test]
async fn only_a_proposed_mirror_group_can_be_dissolved() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-dissolve-refused.sqlite"))
        .await
        .expect("database");
    let declared = release_page(&database, "release", &["Show.720p.mkv", "Show.1080p.mkv"]).await;
    assert_eq!(
        declared[0].mirror.as_ref().expect("a group").source,
        rd_core::MirrorSource::Declared
    );
    assert_eq!(
        database
            .dissolve_mirror_group(declared[0].id)
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::NotProposed
    );
    let ids: Vec<rd_core::CandidateId> = declared.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        grouped_count(&database, &ids).await,
        2,
        "the declared group is still there"
    );

    // And a link that is a mirror of nothing is a request about something that does not exist.
    let lonely = database
        .add_collector_batch(mirror_batch(
            &["https://alone.example/x"],
            vec![Some("Alone.mkv".to_owned())],
            Vec::new(),
        ))
        .await
        .expect("batch")
        .2;
    assert_eq!(
        database
            .dissolve_mirror_group(lonely[0].id)
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::NotGrouped
    );
}

/// A pin and a dissolve never contradict each other (RD-110-34).
///
/// Pinning states which member of a group is fetched; dissolving states there is no group.
/// The second answers a question the first assumed, so the pin goes with the group rather
/// than surviving it as a decision about nothing — and pinning afterwards is refused for the
/// same reason it is refused on any ungrouped link.
#[tokio::test]
async fn dissolving_a_group_takes_its_pin_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-dissolve-pin.sqlite"))
        .await
        .expect("database");
    let pair = proposed_pair(&database, "Show.S01E02").await;
    assert!(
        database
            .set_mirror_pin(pair[1].id, true)
            .await
            .expect("pin")
    );
    assert_eq!(
        database
            .dissolve_mirror_group(pair[1].id)
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::Dissolved
    );
    let ids: Vec<rd_core::CandidateId> = pair.iter().map(|candidate| candidate.id).collect();
    assert_eq!(grouped_count(&database, &ids).await, 0);
    assert!(
        !database
            .set_mirror_pin(pair[1].id, true)
            .await
            .expect("pin again"),
        "a pin on a link that is a mirror of nothing is refused, dissolve or not"
    );
}

/// Pinning a link that is a mirror of nothing is a request about something that does not
/// exist, and it is refused rather than silently succeeding.
#[tokio::test]
async fn pinning_a_link_without_a_group_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-pin-none.sqlite"))
        .await
        .expect("database");
    let candidates = release_page(&database, "lonely", &["Only.One.mkv"]).await;
    assert!(
        candidates[0].mirror.is_none(),
        "a single link is a mirror of nothing"
    );
    assert!(
        !database
            .set_mirror_pin(candidates[0].id, true)
            .await
            .expect("pin")
    );
}
