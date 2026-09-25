use super::{
    Stage, TransferList, boundary_for, form_body, is_safe_id, multipart_body,
    multipart_content_type, permille, place, stage_of,
};

/// All five values of the measurement, each on one state, and an unseen word on a wait.
#[test]
fn every_measured_status_maps_to_one_stage() {
    assert_eq!(stage_of("queued"), Stage::Preparing(30));
    assert_eq!(stage_of("running"), Stage::Working);
    assert_eq!(
        stage_of("finished"),
        Stage::Ready {
            still_running: false
        }
    );
    assert_eq!(
        stage_of("seeding"),
        Stage::Ready {
            still_running: true
        }
    );
    assert_eq!(
        stage_of("error"),
        Stage::Failed(crate::messages::TRANSFER_FAILED)
    );
    assert_eq!(stage_of("something_new"), Stage::Preparing(60));
    assert_eq!(stage_of(""), Stage::Preparing(60));
}

/// `seeding` is ready and not a failure, and it is not `finished` either: the one difference
/// is that the transfer is still running, which is what an empty listing may then mean.
#[test]
fn seeding_is_ready_and_says_that_the_transfer_is_still_running() {
    let Stage::Ready { still_running } = stage_of("seeding") else {
        panic!("seeding is ready");
    };
    assert!(still_running);
    let Stage::Ready { still_running } = stage_of("finished") else {
        panic!("finished is ready");
    };
    assert!(!still_running);
}

/// A fraction, not a percent. Multiplying by ten would finish the bar at one tenth.
#[test]
fn progress_is_a_fraction_from_zero_to_one() {
    assert_eq!(permille(Some(0.0)), Some(0));
    assert_eq!(permille(Some(0.425)), Some(425));
    assert_eq!(permille(Some(1.0)), Some(1000));
    assert_eq!(permille(Some(2.5)), Some(1000), "clamped, not believed");
    assert_eq!(permille(Some(-1.0)), Some(0));
    assert_eq!(permille(Some(f64::NAN)), None);
    assert_eq!(permille(None), None);
}

#[test]
fn a_listing_is_read_with_its_null_fields_intact() {
    let listing: TransferList = serde_json::from_str(
        r#"{"status":"success","transfers":[
            {"id":"REDACTEDTRANSFER01","name":"Example.Release","status":"running",
             "progress":0.425,"message":"downloading","folder_id":null,"file_id":null},
            {"id":"REDACTEDTRANSFER02","name":"one.mkv","status":"finished",
             "folder_id":"REDACTEDFOLDER01","file_id":"REDACTEDFILE01"}]}"#,
    )
    .expect("a listing");
    assert_eq!(listing.transfers.len(), 2);
    assert_eq!(listing.transfers[0].file_id, None);
    assert_eq!(listing.transfers[0].progress, Some(0.425));
    assert_eq!(
        listing.transfers[1].file_id.as_deref(),
        Some("REDACTEDFILE01")
    );
}

#[test]
fn an_identifier_that_could_leave_the_endpoint_is_refused() {
    assert!(is_safe_id("REDACTEDTRANSFER01"));
    assert!(is_safe_id("abc-123_z"));
    for refused in ["", "../account/info", "a/b", "a b", "a?b", &"x".repeat(129)] {
        assert!(!is_safe_id(refused), "{refused}");
    }
}

/// A magnet is full of `&`, `=` and `:`; a body that did not encode them would submit a
/// truncated address.
#[test]
fn a_form_body_encodes_everything_that_is_not_unreserved() {
    assert_eq!(
        String::from_utf8(form_body("src", "magnet:?xt=urn:btih:AA&dn=a b")).expect("ascii"),
        "src=magnet%3A%3Fxt%3Durn%3Abtih%3AAA%26dn%3Da%20b"
    );
    assert_eq!(
        String::from_utf8(form_body("id", "REDACTED-01")).expect("ascii"),
        "id=REDACTED-01"
    );
}

/// The container goes out as an upload with a name, because the name is what tells Premiumize
/// which of its formats it is looking at.
#[test]
fn a_container_becomes_a_multipart_upload_carrying_its_file_name() {
    let boundary = boundary_for("abcdef");
    let body = multipart_body(&boundary, "source.dlc", b"PAYLOAD").expect("a body");
    let text = String::from_utf8(body).expect("ascii");
    assert!(text.starts_with(&format!("--{boundary}\r\n")), "{text}");
    assert!(
        text.contains("Content-Disposition: form-data; name=\"src\"; filename=\"source.dlc\"\r\n"),
        "{text}"
    );
    assert!(text.contains("\r\n\r\nPAYLOAD\r\n"), "{text}");
    assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")), "{text}");
    assert_eq!(
        multipart_content_type(&boundary),
        format!("multipart/form-data; boundary={boundary}")
    );
}

/// A payload carrying the boundary cannot be escaped out of, so it is refused rather than
/// sent as a body that would be read as two parts.
#[test]
fn a_payload_containing_the_boundary_is_refused_rather_than_sent() {
    let boundary = boundary_for("abcdef");
    let payload = format!("before--{boundary}after").into_bytes();
    assert!(multipart_body(&boundary, "source.dlc", &payload).is_none());
}

/// The transfer's name roots the package, and a folder that repeats it is not repeated.
#[test]
fn a_finished_file_keeps_its_name_and_the_place_it_sat_in() {
    assert_eq!(
        place("Example.Release", &[], "ep01.mkv"),
        (
            Some("ep01.mkv".to_owned()),
            Some("Example.Release".to_owned())
        )
    );
    assert_eq!(
        place(
            "Example.Release",
            &["Example.Release".to_owned(), "Sample".to_owned()],
            "sample.mkv"
        ),
        (
            Some("sample.mkv".to_owned()),
            Some("Example.Release/Sample".to_owned())
        ),
        "the transfer name is the root and is never doubled"
    );
    assert_eq!(
        place("", &["..".to_owned(), "Season 1".to_owned()], "ep01.mkv"),
        (Some("ep01.mkv".to_owned()), Some("Season 1".to_owned())),
        "a dot segment cannot leave the package"
    );
    assert_eq!(place("", &[], "  "), (None, None));
}
