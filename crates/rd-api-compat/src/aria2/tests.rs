//! The envelope and the status struct against requests in the shapes AriaNg sends.
//!
//! The router half -- the switch, the secret, the queue behind it -- is
//! `crates/rd-api/tests/sources/compat_aria2.rs`, which replays the same requests end to end.

use rd_core::{DownloadFile, DownloadId, DownloadState};
use serde_json::{Value, json};

use super::{
    methods::METHODS,
    rpc::{self, Request},
    status::{self, List, PackageView},
};

/// AriaNg's first call after connecting.
const GET_VERSION: &str = r#"{"jsonrpc":"2.0","method":"aria2.getVersion","id":"1728550000001","params":["token:s3cret"]}"#;

/// AriaNg's task list poll: the keys it shows, offset and count for the two windowed lists.
const TELL_WAITING: &str = r#"{"jsonrpc":"2.0","method":"aria2.tellWaiting","id":"1728550000002","params":["token:s3cret",0,1000,["gid","totalLength","completedLength","uploadSpeed","downloadSpeed","connections","numSeeders","seeder","status","errorCode","verifiedLength","verifyIntegrityPending","files","bittorrent","infoHash"]]}"#;

/// AriaNg's toolbar pause of two selected tasks: a multicall, the secret inside each call.
const MULTICALL_PAUSE: &str = r#"{"jsonrpc":"2.0","method":"system.multicall","id":"1728550000003","params":[[{"methodName":"aria2.forcePause","params":["token:s3cret","2089b05ecca3d829"]},{"methodName":"aria2.forcePause","params":["token:s3cret","d4d7ab4b0a1f2c3e"]}]]}"#;

/// A batch as browser extensions send one: add a link, then ask for the global figures.
const BATCH_ADD: &str = r#"[{"jsonrpc":"2.0","method":"aria2.addUri","id":1,"params":["token:s3cret",["https://example.test/file.iso"],{"out":"file.iso","pause":"true"}]},{"jsonrpc":"2.0","method":"aria2.getGlobalStat","id":2,"params":["token:s3cret"]}]"#;

fn parsed(body: &str) -> Request {
    rpc::parse(body.as_bytes()).unwrap_or_else(|_| panic!("{body} did not parse"))
}

fn file(state: DownloadState, committed: u64, total: Option<u64>) -> DownloadFile {
    let now = chrono::Utc::now();
    DownloadFile {
        recording: None,
        id: DownloadId::new(),
        package_id: rd_core::PackageId::new(),
        source: "https://example.test/a.bin".parse().expect("url"),
        file_name: "a.bin".to_owned(),
        state,
        total_bytes: total.map(|value| rd_core::ByteCount::new(value).expect("size")),
        committed_bytes: rd_core::ByteCount::new(committed).unwrap_or_default(),
        retry_count: 0,
        next_retry_at: None,
        expected_checksum: None,
        computed_checksum: None,
        last_error: None,
        account_id: None,
        proxy_profile_id: None,
        remote_credential_id: None,
        mirror_group: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        position: 0,
        kind: rd_core::DownloadKind::Http,
        nzb_file_id: None,
        recovery: false,
        media: None,
        enrichment: Vec::new(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn the_secret_is_taken_off_the_parameters_before_a_method_sees_them() {
    let Request::Single(call) = parsed(TELL_WAITING) else {
        panic!("one call");
    };
    assert_eq!(call.method, "aria2.tellWaiting");
    assert_eq!(call.id, json!("1728550000002"));
    assert_eq!(call.token.as_deref(), Some("s3cret"));
    assert_eq!(call.params[0], json!(0));
    assert_eq!(call.params[1], json!(1000));
    assert!(call.params[2].is_array());
    assert_eq!(
        rpc::request_token(&parsed(GET_VERSION)).as_deref(),
        Some("s3cret")
    );
}

#[test]
fn a_multicall_and_a_batch_carry_their_secret_inside_each_call() {
    assert_eq!(
        rpc::request_token(&parsed(MULTICALL_PAUSE)).as_deref(),
        Some("s3cret")
    );
    assert_eq!(
        rpc::request_token(&parsed(BATCH_ADD)).as_deref(),
        Some("s3cret")
    );
    let Request::Single(call) = parsed(MULTICALL_PAUSE) else {
        panic!("one call");
    };
    let entries = rpc::multicall_entries(&call.params).expect("a list of calls");
    let (method, token, params) = entries[1].clone().expect("a call");
    assert_eq!(method, "aria2.forcePause");
    assert_eq!(token.as_deref(), Some("s3cret"));
    assert_eq!(params, vec![json!("d4d7ab4b0a1f2c3e")]);
}

#[test]
fn a_request_with_a_missing_or_a_second_secret_has_none() {
    for body in [
        r#"{"jsonrpc":"2.0","method":"aria2.getVersion","id":1}"#,
        r#"{"jsonrpc":"2.0","method":"aria2.getVersion","id":1,"params":["s3cret"]}"#,
        r#"[{"jsonrpc":"2.0","method":"aria2.getVersion","id":1,"params":["token:a"]},{"jsonrpc":"2.0","method":"aria2.getVersion","id":2,"params":["token:b"]}]"#,
        r#"[{"jsonrpc":"2.0","method":"aria2.getVersion","id":1,"params":["token:a"]},{"jsonrpc":"2.0","method":"aria2.getVersion","id":2}]"#,
        r#"{"jsonrpc":"2.0","method":"system.multicall","id":1,"params":[[]]}"#,
    ] {
        assert_eq!(rpc::request_token(&parsed(body)), None, "{body}");
    }
}

#[test]
fn a_malformed_envelope_is_answered_with_the_json_rpc_codes() {
    for (body, code) in [
        ("{not json", -32700),
        ("42", -32600),
        ("[]", -32600),
        (r#"{"jsonrpc":"2.0","id":1}"#, -32600),
        (
            r#"{"jsonrpc":"2.0","method":"aria2.tellActive","id":1,"params":{}}"#,
            -32602,
        ),
    ] {
        let Err(failure) = rpc::parse(body.as_bytes()) else {
            panic!("{body} parsed");
        };
        assert_eq!(failure.error.code, code, "{body}");
        let response = rpc::single(failure.id, &Err(failure.error));
        assert_eq!(
            response.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "{body}"
        );
    }
    // In a batch the bad element answers in its slot and the good one still runs.
    let Request::Batch(items) = parsed(r#"[{"id":1},{"method":"aria2.getVersion","id":2}]"#) else {
        panic!("a batch");
    };
    assert_eq!(
        items[0]
            .as_ref()
            .map_err(|failure| failure.error.code)
            .err(),
        Some(-32600)
    );
    assert!(items[1].is_ok());
}

#[test]
fn the_answer_objects_are_aria2_s() {
    let ok = rpc::answer(json!("7"), &Ok(json!("OK")));
    assert_eq!(ok, json!({ "id": "7", "jsonrpc": "2.0", "result": "OK" }));
    let refused = rpc::answer(json!(1), &Err(rpc::RpcError::unauthorized()));
    assert_eq!(
        refused["error"],
        json!({ "code": 1, "message": "Unauthorized" })
    );
}

#[test]
fn a_missing_method_is_method_not_found_and_no_server_fault() {
    // AriaNg's "Remove Task" on a finished task met a `500` here in the live test of 1.24
    // (RD-1240-28); a method this subset lacks is the protocol's `-32601`.
    let missing = rpc::RpcError::method_not_found("aria2.changePosition");
    assert_eq!(missing.code, -32601);
    assert_eq!(missing.message, "Method not found: aria2.changePosition");
    let response = rpc::single(json!("ariang"), &Err(missing));
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    // A method that ran and failed stays aria2's code `1`.
    let failed = rpc::single(json!(1), &Err(rpc::RpcError::new("GID x is not found")));
    assert_eq!(
        failed.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn a_gid_is_sixteen_hex_digits_and_finds_its_download_again() {
    let downloads = vec![
        file(DownloadState::Queued, 0, None),
        file(DownloadState::Paused, 0, None),
    ];
    let gid = status::gid(downloads[1].id);
    assert_eq!(gid.len(), 16, "{gid}");
    assert!(gid.chars().all(|digit| digit.is_ascii_hexdigit()), "{gid}");
    assert!(downloads[1].id.to_string().replace('-', "").ends_with(&gid));
    let found = status::find(&downloads, &format!(" {} ", gid.to_ascii_uppercase()));
    assert_eq!(found.map(|download| download.id), Some(downloads[1].id));
    for foreign in ["", "2089b05ecca3d829", "not-a-gid", &gid[..8]] {
        assert!(status::find(&downloads, foreign).is_none(), "{foreign}");
    }
}

#[test]
fn every_state_lands_in_one_of_aria2_s_three_lists() {
    use DownloadState::*;
    for (state, status, list) in [
        (Downloading, "active", List::Active),
        (Extracting, "active", List::Active),
        (Seeding, "active", List::Active),
        (Queued, "waiting", List::Waiting),
        (RetryWait, "waiting", List::Waiting),
        (Blocked, "waiting", List::Waiting),
        (Paused, "paused", List::Waiting),
        (Failed, "error", List::Stopped),
        (Cancelled, "removed", List::Stopped),
        (Completed, "complete", List::Stopped),
    ] {
        assert_eq!(status::status_of(state), status, "{state:?}");
        assert_eq!(status::list_of(state), list, "{state:?}");
    }
}

#[test]
fn the_status_struct_carries_strings_where_aria2_does_and_only_the_asked_keys() {
    let mut download = file(DownloadState::Failed, 512, Some(2048));
    download.last_error = Some(rd_core::Failure::new(
        rd_core::FailureKind::Permanent,
        "could not reach https://cdn.example.test/a.bin?token=s3cr3t-value",
    ));
    let package = PackageView {
        name: "Example",
        destination: "/downloads/Example",
    };
    let entry = status::entry(&download, Some(&package), 0);
    assert_eq!(entry["status"], "error");
    assert_eq!(entry["totalLength"], "2048");
    assert_eq!(entry["completedLength"], "512");
    assert_eq!(entry["downloadSpeed"], "0");
    assert_eq!(entry["dir"], "/downloads/Example");
    assert_eq!(
        entry["files"][0]["uris"][0]["uri"],
        "https://example.test/a.bin"
    );
    let message = entry["errorMessage"].as_str().expect("message");
    assert!(!message.contains("s3cr3t-value"), "{message}");
    assert!(entry.get("bittorrent").is_none());

    let keys = json!(["gid", "status", "numSeeders"]);
    let projected = status::project(entry.clone(), Some(&keys));
    assert_eq!(
        projected
            .as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["gid".to_owned(), "status".to_owned()])
    );
    assert_eq!(
        status::project(entry.clone(), Some(&json!([]))),
        Value::Object(entry)
    );
}

#[test]
fn a_torrent_is_named_by_its_package_and_lists_no_uri() {
    let mut download = file(DownloadState::Seeding, 10, Some(10));
    download.kind = rd_core::DownloadKind::Torrent;
    let package = PackageView {
        name: "Linux ISO",
        destination: "",
    };
    let entry = status::entry(&download, Some(&package), 0);
    assert_eq!(entry["bittorrent"]["info"]["name"], "Linux ISO");
    assert_eq!(entry["seeder"], "true");
    assert_eq!(entry["files"][0]["uris"], json!([]));
    assert_eq!(entry["files"][0]["path"], "a.bin");
}

#[test]
fn a_window_runs_forward_from_the_front_and_backward_from_the_back() {
    let items = [0, 1, 2, 3, 4];
    assert_eq!(status::window(&items, 0, 1000), vec![0, 1, 2, 3, 4]);
    assert_eq!(status::window(&items, 1, 2), vec![1, 2]);
    assert_eq!(status::window(&items, 9, 2), Vec::<i32>::new());
    assert_eq!(status::window(&items, -1, 2), vec![4, 3]);
    assert_eq!(status::window(&items, -5, 3), vec![0]);
    assert_eq!(status::window(&items, -6, 3), Vec::<i32>::new());
    assert_eq!(status::window(&items, 0, -1), Vec::<i32>::new());
}

#[test]
fn list_methods_names_what_the_dispatcher_answers() {
    for method in [
        "aria2.addUri",
        "aria2.tellActive",
        "aria2.tellWaiting",
        "aria2.tellStopped",
        "aria2.tellStatus",
        "aria2.pause",
        "aria2.unpause",
        "aria2.remove",
        "aria2.getGlobalStat",
        "aria2.getVersion",
        "aria2.removeDownloadResult",
        "aria2.purgeDownloadResult",
        "aria2.getFiles",
        "aria2.getOption",
        "aria2.getGlobalOption",
        "aria2.changeOption",
        "aria2.changeGlobalOption",
        "system.listMethods",
    ] {
        assert!(METHODS.contains(&method), "{method}");
    }
}
