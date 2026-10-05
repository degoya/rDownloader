//! What the online check writes onto a candidate: answers, messages, cache hits, fragments.

use rd_core::IngressSource;

use crate::Database;

/// A password somebody appended to an address never reaches a candidate row (RD-109-32).
///
/// RD-108-07 closed this for a link a crawler claims: the fragment becomes an encrypted auth
/// profile and the address is stored bare. A link no crawler claims took the other path and
/// kept its fragment — one typo in the host name was enough to write a share password into
/// `link_candidates.url` in clear text and show it in every LinkGrabber row. Every intake path
/// ends in this writer, so the rule lives here rather than in one of the six handlers.
#[tokio::test]
async fn a_password_in_a_fragment_never_reaches_a_candidate_row() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("fragment.sqlite");
    let database = Database::open(path.clone()).await.expect("database");
    let (_, _, candidates) = database
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
            urls: vec![
                "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret"
                    .parse()
                    .expect("url"),
            ],
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
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].url.as_str(),
        "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4",
        "the stored address is the pasted one without its fragment"
    );

    // The column itself, not the parsed value: this is the row a backup keeps and the
    // interface prints.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("pool");
    let stored: String = sqlx::query_scalar("SELECT url FROM link_candidates")
        .fetch_one(&pool)
        .await
        .expect("stored url");
    pool.close().await;
    assert!(
        !stored.contains("s3cret") && !stored.contains('#'),
        "the candidate row still carries the password: {stored}"
    );
}

/// Two different situations must not reach the reader as one sentence.
///
/// Reported from use (RD-109-43): two links of two different hosters both stood in the
/// LinkGrabber with „Check result missing", `0 B` and the mark `duplicate`. The mark is
/// correct — both addresses had been added before — but the sentence is not: a `duplicate`
/// row can only be reached through the branch that *has* a result, so the plugin had
/// answered, and what it answered was `Unknown`. „Check result missing" is what
/// `link_check_service` passed unconditionally, for the URL a plugin skipped and for the URL
/// a plugin could not judge alike.
#[tokio::test]
async fn an_unknown_answer_and_a_missing_answer_say_different_things() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("check-message.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://1fichier.com/?8x6wertoi51r8vptrojn",
        "https://rapidgator.net/file/4b8480192d90789c5040bb6e43ff4976/Movie.rar.html",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (_batch, _packages, candidates) = database
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

    // The plugin answered about this URL and said it cannot tell.
    database
        .record_candidate_check(
            candidates[0].id,
            Some(rd_core::LinkCheckResult {
                url: candidates[0].url.clone(),
                status: rd_core::LinkStatus::Unknown,
                file_name: None,
                size: None,
                media: None,
            }),
            Some(rd_core::CandidateMessage::coded(
                "collector.check_unknown_no_account",
                "The hoster could not say whether this link is still available",
            )),
            true,
            None,
        )
        .await
        .expect("record");
    // The plugin answered for the batch but said nothing about this URL.
    database
        .record_candidate_check(
            candidates[1].id,
            None,
            Some(rd_core::CandidateMessage::coded(
                "collector.check_no_result",
                "The check returned no answer for this link",
            )),
            false,
            None,
        )
        .await
        .expect("record");

    let listed = database.list_candidates().await.expect("candidates");
    let unknown = listed
        .iter()
        .find(|c| c.id == candidates[0].id)
        .expect("unknown row");
    let missing = listed
        .iter()
        .find(|c| c.id == candidates[1].id)
        .expect("missing row");
    assert_eq!(
        unknown.state,
        rd_core::LinkCandidateState::Duplicate,
        "the reported row: the duplicate mark survives an Unknown answer"
    );
    assert_ne!(
        unknown.error, missing.error,
        "an answer of `Unknown` and no answer at all are not the same thing"
    );
    // The code is what the interface translates; the text is only the English fallback.
    assert_eq!(
        unknown.error_code.as_deref(),
        Some("collector.check_unknown_no_account")
    );
    assert_eq!(
        missing.error_code.as_deref(),
        Some("collector.check_no_result")
    );
    assert_eq!(missing.state, rd_core::LinkCandidateState::Error);
}

/// A conclusive answer clears the message a previous check left behind.
///
/// Without this the sentence and the state beside it contradict each other: a link that was
/// `Unknown` an hour ago and is `online` now would still carry "the hoster could not say".
#[tokio::test]
async fn a_conclusive_answer_clears_the_previous_message() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("check-cleared.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = ["https://1fichier.com/?8x6wertoi51r8vptrojn"]
        .iter()
        .map(|value| value.parse().expect("URL"))
        .collect();
    let (_batch, _packages, candidates) = database
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
    let id = candidates[0].id;
    let url = candidates[0].url.clone();
    database
        .record_candidate_check(
            id,
            Some(rd_core::LinkCheckResult {
                url: url.clone(),
                status: rd_core::LinkStatus::Unknown,
                file_name: None,
                size: None,
                media: None,
            }),
            Some(rd_core::CandidateMessage::coded(
                "collector.check_unknown",
                "The hoster could not say whether this link is still available",
            )),
            false,
            None,
        )
        .await
        .expect("record");
    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .record_candidate_check(
            id,
            Some(rd_core::LinkCheckResult {
                url,
                status: rd_core::LinkStatus::Online,
                file_name: Some("Movie.rar".to_owned()),
                size: rd_core::ByteCount::new(425_123_456).ok(),
                media: None,
            }),
            None,
            false,
            None,
        )
        .await
        .expect("record");
    let listed = database.list_candidates().await.expect("candidates");
    let row = listed.iter().find(|c| c.id == id).expect("row");
    assert_eq!(row.state, rd_core::LinkCandidateState::Online);
    assert_eq!(row.error, None);
    assert_eq!(row.error_code, None);
}

/// RD-120-36: a cache answer is stored as the time of the check, on an `Online` link, and
/// the next check that does not say "cached" clears it. A cache expires without notice, so
/// an old answer must not outlive the check that replaced it.
///
/// RD-130-11: the provider that answered travels with the time, is cleared with it, and is
/// never written without it -- a name next to no time would describe no check.
#[tokio::test]
async fn a_cache_answer_is_a_time_and_the_next_check_clears_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("cached-at.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = vec!["https://www.example.com/file/abc.rar".parse().expect("URL")];
    let (_batch, _packages, candidates) = database
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
    let id = candidates[0].id;
    let answer = |status| rd_core::LinkCheckResult {
        url: candidates[0].url.clone(),
        status,
        file_name: None,
        size: None,
        media: None,
    };

    let before = chrono::Utc::now();
    database
        .record_candidate_check(
            id,
            Some(answer(rd_core::LinkStatus::Cached)),
            None,
            false,
            Some("torbox".to_owned()),
        )
        .await
        .expect("record");
    let cached = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(cached.state, rd_core::LinkCandidateState::Online);
    let cached_at = cached.cached_at.expect("a cache answer carries its time");
    assert!(cached_at >= before);
    assert_eq!(
        Some(cached_at),
        cached.checked_at,
        "the time is the check's own"
    );
    assert_eq!(cached.cached_by.as_deref(), Some("torbox"));

    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .record_candidate_check(
            id,
            Some(answer(rd_core::LinkStatus::Online)),
            None,
            false,
            // A provider handed in without a cached result is not written.
            Some("torbox".to_owned()),
        )
        .await
        .expect("record");
    let rechecked = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(rechecked.state, rd_core::LinkCandidateState::Online);
    assert_eq!(
        rechecked.cached_at, None,
        "an older cache answer does not survive"
    );
    assert_eq!(
        rechecked.cached_by, None,
        "no provider without the time it answered"
    );

    // A link nothing here can check keeps its state and its message, and still carries the
    // cache answer of the provider that holds it.
    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .mark_candidate_unsupported(
            id,
            rd_core::CandidateMessage::coded(
                "collector.check_no_source",
                "no service can check this address",
            ),
            Some("torbox".to_owned()),
        )
        .await
        .expect("unsupported");
    let unsupported = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(unsupported.state, rd_core::LinkCandidateState::Unsupported);
    assert_eq!(
        unsupported.error_code.as_deref(),
        Some("collector.check_no_source")
    );
    assert!(unsupported.cached_at.is_some());
    assert_eq!(unsupported.cached_by.as_deref(), Some("torbox"));

    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .mark_candidate_unsupported(
            id,
            rd_core::CandidateMessage::coded(
                "collector.check_no_source",
                "no service can check this address",
            ),
            None,
        )
        .await
        .expect("unsupported");
    let cleared = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(cleared.cached_at, None);
    assert_eq!(cleared.cached_by, None);
}
