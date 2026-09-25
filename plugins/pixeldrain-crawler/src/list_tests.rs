//! What is claimed, and what a list answer becomes. Nothing opens a socket; every fixture body
//! is sanitised, with invented identifiers and no address or account of a real person.

use super::{Child, FileList, children, file_url, list_id, list_url, parse, refusal_token};

fn list(json: &str) -> FileList {
    parse(json.as_bytes()).expect("a JSON object")
}

#[test]
fn the_two_list_shapes_are_claimed_and_a_file_address_is_not() {
    assert_eq!(
        list_id("https://pixeldrain.com/l/Lm4pQ2").as_deref(),
        Some("Lm4pQ2")
    );
    assert_eq!(
        list_id("https://www.pixeldrain.com/api/list/Lm4pQ2").as_deref(),
        Some("Lm4pQ2")
    );
    // Exactly one of the two plugins answers for any address: a file belongs to
    // `plugins/pixeldrain/`, so it is left alone here.
    assert_eq!(list_id("https://pixeldrain.com/u/Ab3xY9Zq"), None);
    assert_eq!(list_id("https://pixeldrain.com/api/file/Ab3xY9Zq"), None);
    assert_eq!(list_id("https://pixeldrain.com.evil.test/l/Lm4pQ2"), None);
    assert_eq!(list_id("https://pixeldrain.com/l/../../etc/passwd"), None);
    assert_eq!(list_id("not an address"), None);
    assert_eq!(list_url("Lm4pQ2"), "https://pixeldrain.com/api/list/Lm4pQ2");
    assert_eq!(file_url("Ab3xY9Zq"), "https://pixeldrain.com/u/Ab3xY9Zq");
}

#[test]
fn a_list_becomes_one_candidate_per_file_under_the_lists_own_title() {
    let answer = list(
        r#"{"success":true,"id":"Lm4pQ2","title":"Season 1","file_count":2,
            "files":[{"id":"Ab3xY9Zq","name":"a.bin","size":4096},
                     {"id":"Cd7wV1Nk","name":"b.bin","size":8192}]}"#,
    );
    assert_eq!(
        children(&answer),
        vec![
            Child {
                url: "https://pixeldrain.com/u/Ab3xY9Zq".to_owned(),
                file_name: Some("a.bin".to_owned()),
                size: Some(4096),
                package_hint: Some("Season 1".to_owned()),
            },
            Child {
                url: "https://pixeldrain.com/u/Cd7wV1Nk".to_owned(),
                file_name: Some("b.bin".to_owned()),
                size: Some(8192),
                package_hint: Some("Season 1".to_owned()),
            },
        ]
    );
}

#[test]
fn an_entry_without_a_usable_identifier_contributes_nothing() {
    // There is no address to build from it, and inventing one would put a row in the
    // LinkGrabber that can never resolve.
    let answer = list(
        r#"{"success":true,"title":"   ",
            "files":[{"name":"a.bin"},{"id":"","name":"b.bin"},
                     {"id":"../../etc/passwd"},{"id":"Ab3xY9Zq","name":"   "}]}"#,
    );
    let entries = children(&answer);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].url, "https://pixeldrain.com/u/Ab3xY9Zq");
    // A blank name is dropped rather than turned into a file called nothing, and a blank title
    // is no package hint at all.
    assert_eq!(entries[0].file_name, None);
    assert_eq!(entries[0].package_hint, None);
}

#[test]
fn a_refusal_carries_its_stable_token_and_never_the_providers_sentence() {
    let answer = list(
        r#"{"success":false,"value":"list_not_found","message":"The list you requested is gone."}"#,
    );
    assert_eq!(refusal_token(&answer).as_deref(), Some("list_not_found"));
    assert!(children(&answer).is_empty());

    // Not code-shaped: nothing to branch on and nothing safe to show, so the whole value goes
    // rather than being filtered -- a value that echoed a request would keep whatever survived.
    let prose = list(r#"{"success":false,"value":"Slow down, https://pixeldrain.com/l/x"}"#);
    assert_eq!(refusal_token(&prose).as_deref(), Some(""));

    // A successful answer carries no token at all.
    let good = list(r#"{"success":true,"files":[]}"#);
    assert_eq!(refusal_token(&good), None);
}

#[test]
fn an_array_answer_is_never_mistaken_for_a_list() {
    // Serde reads a struct out of a sequence in field order, so an unguarded parse would invent
    // `success` and `value` out of the first two elements.
    assert!(parse(br#"["a","b"]"#).is_none());
    assert!(parse(b"<html>maintenance</html>").is_none());
}
