//! What Put.io's answers mean, pinned down on the host target.

use putio_common::reason::ErrorEnvelope;

use super::{
    ErrorKind, Stage, Transfer, TransferListResponse, TransferResponse, cancel_body, failure_from,
    is_safe_remote_id, package_hint, permille, rate_limit_wait, stage_of,
};
use crate::messages;

fn envelope(body: &str) -> ErrorEnvelope {
    ErrorEnvelope::of(body.as_bytes())
}

#[test]
fn a_transfer_is_read_with_what_the_sweep_needs_of_it() {
    let response: TransferResponse = serde_json::from_str(
        r#"{"transfer":{"id":77,"name":"Example.Release","status":"DOWNLOADING",
            "percent_done":42,"down_speed":1048576,"estimated_time":120,"file_id":null,
            "hash":"da39a3ee5e6b4b0d3255bfef95601890afd80709"},"status":200}"#,
    )
    .expect("a transfer document");
    let transfer = response.transfer.expect("the transfer");
    assert_eq!(transfer.id, Some(77));
    assert_eq!(transfer.status.as_deref(), Some("DOWNLOADING"));
    assert_eq!(permille(transfer.percent_done), Some(420));
    assert_eq!(transfer.down_speed, Some(1_048_576));
    assert_eq!(transfer.estimated_time, Some(120));
}

/// Put.io's ten states, mapped onto the four the contract has. The two that are not obvious
/// carry their reasoning in `stage_of`; this is what pins them down.
#[test]
fn put_ios_states_map_onto_the_contracts_four() {
    assert_eq!(stage_of("DOWNLOADING"), Stage::Working);
    // Complete is complete: seeding is Put.io's business, not a reason to make somebody wait.
    assert_eq!(stage_of("COMPLETED"), Stage::Ready);
    assert_eq!(stage_of("SEEDING"), Stage::Ready);
    assert_eq!(stage_of("IN_QUEUE"), Stage::Preparing(30));
    assert_eq!(stage_of("WAITING"), Stage::Preparing(30));
    assert_eq!(stage_of("PREPARING"), Stage::Preparing(10));
    assert_eq!(stage_of("COMPLETING"), Stage::Preparing(15));
    assert_eq!(stage_of("ERROR"), Stage::Failed(messages::TRANSFER_FAILED));
    assert_eq!(
        stage_of("CANCELLED"),
        Stage::Failed(messages::TRANSFER_CANCELLED)
    );
    // A word this build has never heard of is somebody's transfer going perfectly well.
    assert_eq!(stage_of("SOMETHING_NEW"), Stage::Preparing(60));
    assert_eq!(stage_of(""), Stage::Preparing(60));
}

#[test]
fn a_percentage_becomes_thousandths_and_nothing_outside_them() {
    assert_eq!(permille(Some(0)), Some(0));
    assert_eq!(permille(Some(100)), Some(1000));
    // Put.io has answered both of these; neither is believed as stated.
    assert_eq!(permille(Some(-5)), Some(0));
    assert_eq!(permille(Some(4000)), Some(1000));
    assert_eq!(permille(None), None);
}

/// The window `adopt` looks through. Put.io fills a different field for a magnet, for an
/// uploaded torrent and for a transfer it created itself, so all three are asked.
#[test]
fn a_transfer_is_recognised_by_whichever_field_put_io_filled() {
    let key = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    let response: TransferListResponse = serde_json::from_str(
        r#"{"transfers":[
            {"id":1,"hash":"DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"},
            {"id":2,"magneturi":"magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709"},
            {"id":3,"source":"magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=x"},
            {"id":4,"hash":"ffffffffffffffffffffffffffffffffffffffff"},
            {"id":5,"source":"https://example.invalid/file.bin"}
        ]}"#,
    )
    .expect("a transfer list");
    let matched: Vec<i64> = response
        .transfers
        .iter()
        .filter(|transfer| transfer.carries(key))
        .filter_map(|transfer| transfer.id)
        .collect();
    assert_eq!(matched, vec![1, 2, 3], "a stranger's transfer is not ours");
}

/// A transfer naming nothing at all matches nothing. Without this an account whose transfers
/// carry no hash would adopt the first one in the list.
#[test]
fn a_transfer_that_names_no_torrent_is_never_adopted() {
    let empty = Transfer::default();
    assert!(!empty.carries("da39a3ee5e6b4b0d3255bfef95601890afd80709"));
}

/// The identifier goes back out in a request path, so it is checked rather than trusted.
#[test]
fn only_a_put_io_transfer_id_reaches_a_request_path() {
    assert!(is_safe_remote_id("77"));
    assert!(is_safe_remote_id("18446744073709551"));
    for hostile in [
        "",
        "../account/info",
        "77/../../account",
        "77?x=1",
        "abc",
        "7 7",
        &"9".repeat(21),
    ] {
        assert!(!is_safe_remote_id(hostile), "{hostile}");
    }
}

#[test]
fn a_package_hint_is_the_folders_a_file_sat_in() {
    assert_eq!(
        package_hint(&["Example.Release".to_owned(), "Sample".to_owned()]).as_deref(),
        Some("Example.Release/Sample")
    );
    // A loose file sat in nothing, and that is not a folder called "".
    assert_eq!(package_hint(&[]), None);
    assert_eq!(package_hint(&["  ".to_owned()]), None);
    // A folder Put.io named `..` is not an escape upwards.
    assert_eq!(
        package_hint(&["..".to_owned(), "Example".to_owned()]).as_deref(),
        Some("Example")
    );
}

#[test]
fn the_cancel_body_names_one_transfer() {
    assert_eq!(cancel_body("77"), b"transfer_ids=77".to_vec());
}

#[test]
fn a_plain_success_is_not_a_refusal() {
    assert!(failure_from(200, None, &envelope(r#"{"transfer":{"id":1}}"#)).is_none());
    assert!(failure_from(204, None, &envelope("")).is_none());
}

/// The refusals a person acts on differently.
#[test]
fn the_refusals_are_told_apart_by_what_a_person_has_to_do() {
    let expired =
        failure_from(401, None, &envelope(r#"{"error_type":"INVALID_TOKEN"}"#)).expect("a refusal");
    assert_eq!(expired.kind, ErrorKind::AccountInvalid);
    assert_eq!(expired.code, messages::AUTH_INVALID.0);
    assert_eq!(
        expired.params,
        vec![("reason", "INVALID_TOKEN".to_owned())],
        "the stable word travels and the sentence does not"
    );

    // A 403 is two different things at Put.io, and only the word tells them apart.
    let refused =
        failure_from(403, None, &envelope(r#"{"error_type":"ACCESS_DENIED"}"#)).expect("a refusal");
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert_eq!(refused.code, messages::NOT_PERMITTED.0);
    let stale =
        failure_from(403, None, &envelope(r#"{"error_type":"INVALID_GRANT"}"#)).expect("a refusal");
    assert_eq!(stale.kind, ErrorKind::AccountInvalid);

    let gone =
        failure_from(404, None, &envelope(r#"{"error_type":"NOT_FOUND"}"#)).expect("a refusal");
    assert_eq!(gone.kind, ErrorKind::Offline);
    assert_eq!(gone.code, messages::TRANSFER_GONE.0);

    let busy = failure_from(502, None, &envelope("")).expect("a refusal");
    assert_eq!(busy.kind, ErrorKind::Transient(Some(300)));
}

/// A full account is not "Put.io said no": it is one thing the person can actually fix, and it
/// arrives under whatever status Put.io felt like using.
#[test]
fn a_full_account_says_so_whatever_status_it_arrives_under() {
    for status in [400, 403, 413] {
        let refusal = failure_from(
            status,
            None,
            &envelope(r#"{"error_type":"DISK_QUOTA_EXCEEDED"}"#),
        )
        .expect("a refusal");
        assert_eq!(refusal.code, messages::DISK_FULL.0, "{status}");
        assert_eq!(refusal.kind, ErrorKind::Permanent, "{status}");
    }
}

/// A rate limit is a wait, with Put.io's own window when the two clocks agree on one.
#[test]
fn a_spent_request_budget_is_a_wait_and_not_a_failure() {
    let waiting = failure_from(429, Some(120), &envelope("")).expect("a refusal");
    assert_eq!(waiting.kind, ErrorKind::RateLimited(Some(120)));
    assert_eq!(waiting.code, messages::RATE_LIMITED.0);
    let default = failure_from(429, None, &envelope("")).expect("a refusal");
    assert_eq!(default.kind, ErrorKind::RateLimited(Some(60)));
}

#[test]
fn only_a_reset_the_clocks_agree_on_becomes_a_wait() {
    assert_eq!(rate_limit_wait(Some("1000060"), 1_000_000), Some(60));
    assert_eq!(rate_limit_wait(Some("999999"), 1_000_000), None);
    assert_eq!(rate_limit_wait(Some("soon"), 1_000_000), None);
    assert_eq!(rate_limit_wait(Some("1086400"), 1_000_000), None);
    assert_eq!(rate_limit_wait(None, 1_000_000), None);
}

/// Put.io answers some refusals with a 2xx and an error document. Believing the status would
/// hand the sweep an empty transfer and call it progress.
#[test]
fn an_error_document_decides_whatever_the_status_says() {
    let refusal = failure_from(
        200,
        None,
        &envelope(r#"{"error_type":"DATABASE_ERROR","error_message":"try again later"}"#),
    )
    .expect("a refusal");
    assert_eq!(refusal.code, messages::API_ERROR.0);
    assert_eq!(
        refusal.params,
        vec![("reason", "DATABASE_ERROR".to_owned())]
    );
    assert!(!refusal.message.contains("try again"));
}
