use super::{
    ErrorEnvelope, ErrorKind, Stage, TorrentFile, classify_error, ensure_http_status, failure_from,
    is_selected, magnet_body, pair_links, permille, selection_body, stage_of,
};
use crate::messages;

fn file(id: i64, path: &str, selected: i64) -> TorrentFile {
    TorrentFile {
        id: Some(id),
        path: Some(path.to_owned()),
        bytes: Some(1_024),
        selected: Some(selected),
    }
}

/// The one state that makes this a remote job rather than a crawl: Real-Debrid stops and
/// waits for a person, and nothing it answers afterwards carries a link until they have
/// chosen.
#[test]
fn waiting_for_a_selection_is_its_own_stage() {
    assert_eq!(stage_of("waiting_files_selection"), Stage::AwaitingChoice);
}

/// Every documented state maps to something, and the three that are not failures are worth
/// pinning: `queued` is not "downloading at zero percent", and the two post-processing states
/// are not "finished".
#[test]
fn every_documented_torrent_state_maps_to_a_stage() {
    assert!(matches!(stage_of("magnet_conversion"), Stage::Preparing(_)));
    assert!(matches!(stage_of("queued"), Stage::Preparing(_)));
    assert!(matches!(stage_of("compressing"), Stage::Preparing(_)));
    assert!(matches!(stage_of("uploading"), Stage::Preparing(_)));
    assert!(matches!(stage_of("downloading"), Stage::Working(_)));
    assert_eq!(stage_of("downloaded"), Stage::Ready);
    assert_eq!(
        stage_of("magnet_error"),
        Stage::Failed(messages::MAGNET_REJECTED)
    );
    assert_eq!(stage_of("dead"), Stage::Failed(messages::TORRENT_DEAD));
    assert_eq!(stage_of("virus"), Stage::Failed(messages::CONTENT_REFUSED));
    assert_eq!(stage_of("error"), Stage::Failed(messages::TORRENT_FAILED));
}

/// A word this build does not know is not a reason to throw a torrent away. Real-Debrid has
/// added states before, and failing on one would end a job that was going perfectly well.
#[test]
fn an_unknown_state_waits_instead_of_failing() {
    assert!(matches!(stage_of("some_future_state"), Stage::Preparing(_)));
    assert!(matches!(stage_of(""), Stage::Preparing(_)));
}

/// Percent in, thousandths out, and nothing that is not a number gets through.
#[test]
fn progress_is_converted_and_bounded() {
    assert_eq!(permille(Some(0.0)), Some(0));
    assert_eq!(permille(Some(37.5)), Some(375));
    assert_eq!(permille(Some(100.0)), Some(1_000));
    // A provider answering past the end is clamped rather than believed.
    assert_eq!(permille(Some(140.0)), Some(1_000));
    assert_eq!(permille(Some(-5.0)), Some(0));
    assert_eq!(permille(Some(f64::NAN)), None);
    assert_eq!(permille(None), None);
}

/// Real-Debrid says nothing about which link belongs to which file beyond their order, so the
/// pairing is positional — and when the two lists do not line up, a link keeps its address and
/// loses its name rather than borrowing somebody else's.
#[test]
fn links_are_paired_with_the_selected_files_and_never_guessed() {
    let files = vec![
        file(1, "/Show/a.mkv", 1),
        file(2, "/Show/sample.mkv", 0),
        file(3, "/Show/b.mkv", 1),
    ];
    let links = vec!["https://rd/1".to_owned(), "https://rd/3".to_owned()];
    let paired = pair_links(&links, &files);
    assert_eq!(paired.len(), 2);
    assert_eq!(paired[0].1.and_then(|f| f.id), Some(1));
    assert_eq!(paired[1].1.and_then(|f| f.id), Some(3));

    // One link too many: nothing is paired, because any pairing would be a guess.
    let mismatched = vec![
        "https://rd/1".to_owned(),
        "https://rd/3".to_owned(),
        "https://rd/9".to_owned(),
    ];
    let paired = pair_links(&mismatched, &files);
    assert_eq!(paired.len(), 3);
    assert!(paired.iter().all(|(_, file)| file.is_none()));
}

#[test]
fn a_selection_is_sent_as_the_providers_own_ids() {
    assert_eq!(selection_body(&[3, 1, 3]), b"files=1,3".to_vec());
    assert!(is_selected(Some(1)));
    assert!(!is_selected(Some(0)));
    assert!(!is_selected(None));
}

/// A magnet is full of `&`, `=` and `:`. A body that did not encode them would submit a
/// truncated address and create a torrent nobody asked for.
#[test]
fn a_magnet_body_is_encoded_rather_than_pasted() {
    let body = magnet_body("magnet:?xt=urn:btih:abc&dn=a b");
    let text = String::from_utf8(body).expect("ascii");
    assert!(text.starts_with("magnet="));
    assert!(!text[7..].contains('&'), "{text}");
    assert!(!text[7..].contains('='), "{text}");
    assert!(text.contains("%3A"), "{text}");
    assert!(text.contains("%20"), "{text}");
}

/// The provider's number decides, the provider's sentence never does.
#[test]
fn a_refusal_is_classified_by_its_number_and_never_by_its_sentence() {
    let failure = classify_error(22, None);
    assert_eq!(failure.kind, ErrorKind::IpBlocked);
    assert_eq!(failure.code, messages::IP_NOT_ALLOWED.0);
    assert_eq!(failure.params, vec![("api_code", "22".to_owned())]);
    assert!(
        !failure.message.contains("http"),
        "the provider's own text must not travel: {}",
        failure.message
    );
    // 5 and 34 are the same wait, because refused requests count towards the cap that
    // refused them.
    assert_eq!(
        classify_error(5, Some(90)).kind,
        ErrorKind::RateLimited(Some(90))
    );
    assert_eq!(
        classify_error(34, None).kind,
        ErrorKind::RateLimited(Some(60))
    );
    // A number with no bucket is permanent and carries the number, not a sentence.
    let unknown = classify_error(9_999, None);
    assert_eq!(unknown.kind, ErrorKind::Permanent);
    assert_eq!(unknown.code, messages::API_ERROR.0);
}

/// A 2xx carrying an `error_code` is a refusal, and a 4xx carrying none is classified by its
/// status alone. Reading only the status would let the first through.
#[test]
fn a_success_status_carrying_an_error_code_is_still_a_refusal() {
    let envelope = ErrorEnvelope {
        error: Some("Permission denied".to_owned()),
        error_code: Some(9),
    };
    let failure = failure_from(200, None, &envelope).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);

    let empty = ErrorEnvelope::default();
    assert!(failure_from(200, None, &empty).is_none());
    assert!(failure_from(503, None, &empty).is_some());
    assert!(ensure_http_status(204, None).is_ok());
    assert!(ensure_http_status(451, None).is_err());
}

/// The torrent's own name is the root of every path below it, which is what turns the magnet
/// somebody pasted into the package they expect.
#[test]
fn a_finished_file_keeps_its_place_inside_the_torrent() {
    // Real-Debrid roots a multi-file torrent's paths at the torrent's own name, so that
    // name appears once in the place and not twice.
    let (name, hint) = super::place("Show.Name.S01", "/Show.Name.S01/Season 1/ep01.mkv");
    assert_eq!(name.as_deref(), Some("ep01.mkv"));
    assert_eq!(hint.as_deref(), Some("Show.Name.S01/Season 1"));
    // A path rooted somewhere else keeps the torrent's name in front of it.
    let (name, hint) = super::place("Show.Name.S01", "/Extras/notes.txt");
    assert_eq!(name.as_deref(), Some("notes.txt"));
    assert_eq!(hint.as_deref(), Some("Show.Name.S01/Extras"));
    // A single-file torrent has no sub-path; the torrent's name is the package.
    let (name, hint) = super::place("film.mkv", "/film.mkv");
    assert_eq!(name.as_deref(), Some("film.mkv"));
    assert_eq!(hint.as_deref(), Some("film.mkv"));
    // Dot segments in a stranger's path never become part of a place on disk.
    let (name, hint) = super::place("job", "/../../etc/passwd");
    assert_eq!(name.as_deref(), Some("passwd"));
    assert_eq!(hint.as_deref(), Some("job/etc"));
}

/// The identifier comes back from the provider and goes out again in a URL. One carrying a
/// slash would be a request to somewhere else on the only host this plugin may reach.
#[test]
fn a_provider_identifier_is_checked_before_it_goes_into_a_path() {
    assert!(super::is_safe_remote_id("ABCD1234"));
    assert!(super::is_safe_remote_id("a-b_c"));
    assert!(!super::is_safe_remote_id(""));
    assert!(!super::is_safe_remote_id("../user"));
    assert!(!super::is_safe_remote_id("ab/cd"));
    assert!(!super::is_safe_remote_id(&"x".repeat(129)));
}
