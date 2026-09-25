use super::*;

/// The folder listing measured on 2026-09-22, trimmed to what a parser reads.
const LISTING: &str = r#"[{"f":[
 {"h":"G5NikTgR","p":"39FwkLpK","t":1,"a":"rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ","k":"G5NikTgR:pR93bkC1OGslo_O5ugTeWw","ts":1632475428},
 {"h":"KlVgwR4B","p":"G5NikTgR","t":0,"a":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","k":"G5NikTgR:IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q","s":523265,"ts":1632475461},
 {"h":"zwNiSB7J","p":"G5NikTgR","t":1,"a":"bAMOwUGKrJzOHaWpoJEta9ATY54OrnrM1MdM18UevI4","k":"G5NikTgR:jRCDoNOtdI1WwR6-rOtbWg/zwNiSB7J:Gc71mTjFO44hyqSjItIL9g","ts":1632475524}
],"sn":"54_AmP_AxTw","noc":1}]"#;

#[test]
fn a_failure_is_read_whether_it_is_wrapped_or_bare() {
    // Both shapes measured: `[{"a":"g",…}]` answers `[-9]`, `?n=AAAAAAAA` answers `-9`.
    assert_eq!(first_object(b"[-9]"), Err(Some(ENOENT)));
    assert_eq!(first_object(b"-9"), Err(Some(ENOENT)));
    assert_eq!(first_object(b"[-15]"), Err(Some(ESID)));
}

#[test]
fn a_body_that_is_neither_is_not_read_for_values() {
    assert_eq!(first_object(b"<html>down</html>"), Err(None));
    assert_eq!(first_object(b"[]"), Err(None));
    assert_eq!(first_object(&[0xff, 0xfe]), Err(None));
}

#[test]
fn the_download_answer_gives_an_address_a_size_and_attributes() {
    let answer = first_object(
        br#"[{"s":10000000,"at":"TKoe","g":"https://gfs262n326.userstorage.mega.co.nz/dl/x"}]"#,
    )
    .expect("object");
    let (url, size, attributes) = download_target(&answer).expect("complete answer");
    assert!(url.ends_with(".userstorage.mega.co.nz/dl/x"));
    assert_eq!(size, 10_000_000);
    assert_eq!(attributes, "TKoe");
}

#[test]
fn an_answer_without_an_address_is_not_a_download() {
    let answer = first_object(br#"[{"s":10000000,"at":"TKoe"}]"#).expect("object");
    assert!(download_target(&answer).is_none());
}

#[test]
fn the_listing_yields_its_nodes_and_its_root() {
    let answer = first_object(LISTING.as_bytes()).expect("object");
    let nodes = Node::list(&answer);
    assert_eq!(nodes.len(), 3);
    let root = share_root(&nodes).expect("a root");
    assert_eq!(root.handle, "G5NikTgR");
    assert_eq!(root.kind, 1);
    let file = nodes.iter().find(|node| node.kind == 0).expect("a file");
    assert_eq!(file.size, 523_265);
    assert_eq!(
        file.key_under("G5NikTgR"),
        Some("IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q")
    );
}

#[test]
fn a_node_shared_under_its_own_handle_keeps_the_entries_apart() {
    let answer = first_object(LISTING.as_bytes()).expect("object");
    let nodes = Node::list(&answer);
    let nested = nodes
        .iter()
        .find(|node| node.handle == "zwNiSB7J")
        .expect("nested folder");
    assert_eq!(nested.key_under("G5NikTgR"), Some("jRCDoNOtdI1WwR6-rOtbWg"));
    assert_eq!(nested.key_under("zwNiSB7J"), Some("Gc71mTjFO44hyqSjItIL9g"));
    assert_eq!(nested.key_under("nobody"), None);
}

#[test]
fn requests_are_the_shapes_that_were_measured() {
    assert_eq!(
        String::from_utf8(file_request("yuZ0QJ6J")).expect("ascii"),
        r#"[{"a":"g","g":1,"ssl":2,"p":"yuZ0QJ6J"}]"#
    );
    assert_eq!(
        String::from_utf8(folder_child_request("KlVgwR4B")).expect("ascii"),
        r#"[{"a":"g","g":1,"ssl":2,"n":"KlVgwR4B"}]"#
    );
    assert_eq!(
        endpoint(Some("e4diDZ7T")),
        "https://g.api.mega.co.nz/cs?n=e4diDZ7T"
    );
    assert_eq!(endpoint(None), "https://g.api.mega.co.nz/cs");
}

#[test]
fn a_handle_is_escaped_rather_than_pasted() {
    let body = String::from_utf8(file_request("a\"b")).expect("ascii");
    assert!(body.contains(r#""p":"a\"b""#), "{body}");
}

// -- what a refusal's headers say about waiting ---------------------------

#[test]
fn megas_own_header_is_read_before_the_standard_one() {
    let headers = vec![
        ("Retry-After".to_owned(), "30".to_owned()),
        ("X-Mega-Time-Left".to_owned(), "1800".to_owned()),
    ];
    assert_eq!(retry_after(&headers), Some(1800));
}

#[test]
fn the_standard_header_is_read_when_mega_sent_none() {
    let headers = vec![("retry-after".to_owned(), " 45 ".to_owned())];
    assert_eq!(retry_after(&headers), Some(45));
}

/// The date form of `Retry-After` needs a clock to subtract from and a guest has none. It is
/// reported as "no number", not as zero -- a zero would be a scheduler retrying at once.
#[test]
fn a_retry_after_date_is_not_guessed_at() {
    let headers = vec![(
        "Retry-After".to_owned(),
        "Wed, 21 Oct 2026 07:28:00 GMT".to_owned(),
    )];
    assert_eq!(retry_after(&headers), None);
}

#[test]
fn a_refusal_with_no_such_header_asks_for_no_particular_wait() {
    assert_eq!(
        retry_after(&[("Server".to_owned(), "nginx".to_owned())]),
        None
    );
}
