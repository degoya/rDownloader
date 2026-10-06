use super::{
    connect_request, is_stale, parse_connect_response, parse_http_scrape, parse_udp_scrape,
    scrape_request, scrape_url,
};

#[test]
fn scrape_urls_follow_the_announce_convention() {
    assert_eq!(
        scrape_url("https://tracker.example/announce").as_deref(),
        Some("https://tracker.example/scrape")
    );
    // A passkey in the path is preserved.
    assert_eq!(
        scrape_url("https://tracker.example/abc123/announce").as_deref(),
        Some("https://tracker.example/abc123/scrape")
    );
    // BEP 48 allows a suffix after `announce`.
    assert_eq!(
        scrape_url("https://tracker.example/announce.php").as_deref(),
        Some("https://tracker.example/scrape.php")
    );
}

#[test]
fn a_tracker_without_an_announce_path_has_no_scrape_endpoint() {
    assert!(scrape_url("https://tracker.example/tr").is_none());
    assert!(scrape_url("not a url").is_none());
}

#[test]
fn http_counters_are_read_for_the_requested_hash() {
    let hash = [1_u8; 20];
    let mut body = b"d5:filesd20:".to_vec();
    body.extend_from_slice(&hash);
    body.extend_from_slice(b"d8:completei12e10:downloadedi34e10:incompletei5eeee");
    let scrape = parse_http_scrape(&body, &hash).expect("parses");
    assert_eq!(scrape.seeders, 12);
    assert_eq!(scrape.completed, 34);
    assert_eq!(scrape.leechers, 5);
}

#[test]
fn a_tracker_failure_is_surfaced_rather_than_read_as_zero() {
    let error =
        parse_http_scrape(b"d14:failure reason9:not founde", &[0_u8; 20]).expect_err("fails");
    assert!(error.to_string().contains("not found"));
}

#[test]
fn the_udp_connect_handshake_round_trips() {
    let request = connect_request(0xdead_beef);
    assert_eq!(&request[..8], &0x0417_2710_1980_u64.to_be_bytes());
    assert_eq!(&request[8..12], &0_u32.to_be_bytes());

    let mut response = Vec::new();
    response.extend_from_slice(&0_u32.to_be_bytes());
    response.extend_from_slice(&0xdead_beef_u32.to_be_bytes());
    response.extend_from_slice(&0x0102_0304_0506_0708_u64.to_be_bytes());
    assert_eq!(
        parse_connect_response(&response, 0xdead_beef).expect("parses"),
        0x0102_0304_0506_0708
    );
}

#[test]
fn a_mismatched_transaction_id_is_refused() {
    let mut response = Vec::new();
    response.extend_from_slice(&0_u32.to_be_bytes());
    response.extend_from_slice(&1_u32.to_be_bytes());
    response.extend_from_slice(&0_u64.to_be_bytes());
    assert!(parse_connect_response(&response, 2).is_err());
}

#[test]
fn udp_scrape_counters_are_read_in_protocol_order() {
    let request = scrape_request(7, 9, &[3_u8; 20]);
    assert_eq!(request.len(), 36);
    assert_eq!(&request[8..12], &2_u32.to_be_bytes());

    let mut response = Vec::new();
    response.extend_from_slice(&2_u32.to_be_bytes());
    response.extend_from_slice(&9_u32.to_be_bytes());
    response.extend_from_slice(&11_u32.to_be_bytes());
    response.extend_from_slice(&22_u32.to_be_bytes());
    response.extend_from_slice(&33_u32.to_be_bytes());
    let scrape = parse_udp_scrape(&response, 9).expect("parses");
    // BEP 15 orders the counters seeders, completed, leechers.
    assert_eq!(scrape.seeders, 11);
    assert_eq!(scrape.completed, 22);
    assert_eq!(scrape.leechers, 33);
}

#[test]
fn a_truncated_udp_response_is_refused() {
    assert!(parse_udp_scrape(&[0, 0, 0, 2], 9).is_err());
}

#[tokio::test]
async fn a_tracker_pointing_at_an_internal_address_is_refused() {
    // The shapes that make a hostile `.torrent` an SSRF primitive.
    for announce in [
        "http://127.0.0.1:8710/announce",
        "http://localhost/announce",
        "http://169.254.169.254/announce",
        "http://0.0.0.0/announce",
        "http://[::1]/announce",
        // An IPv4 loopback address in IPv6 clothing, which the tracker check alone used
        // to let through.
        "http://[::ffff:127.0.0.1]/announce",
        "udp://[fe80::1]:6969/announce",
    ] {
        let url = url::Url::parse(announce).expect("valid url");
        let refused = super::reject_internal_target(&url).await;
        assert!(refused.is_err(), "{announce} was not refused");
    }
}

#[tokio::test]
async fn a_tracker_on_the_local_network_stays_allowed() {
    // A self-hosted tracker on the LAN is a real setup; only this machine is refused.
    for announce in [
        "http://192.168.1.10:6969/announce",
        "udp://10.0.0.2:6969/announce",
        "http://[fd00::2]/announce",
    ] {
        let url = url::Url::parse(announce).expect("valid url");
        assert!(
            super::reject_internal_target(&url).await.is_ok(),
            "{announce} was refused"
        );
    }
}

#[tokio::test]
async fn a_host_that_does_not_resolve_is_refused_rather_than_attempted() {
    let url = url::Url::parse("http://tracker.invalid/announce").expect("valid url");
    assert!(super::reject_internal_target(&url).await.is_err());
}

#[test]
fn a_hostile_tracker_message_cannot_flood_the_error_field() {
    let long = "x".repeat(5_000);
    let body = format!("d14:failure reason{}:{}e", long.len(), long);
    let error = parse_http_scrape(body.as_bytes(), &[0_u8; 20]).expect_err("fails");
    assert!(error.to_string().len() < 400);
}

#[test]
fn counters_go_stale_after_the_freshness_window() {
    let now = chrono::Utc::now();
    assert!(!is_stale(now - chrono::Duration::minutes(5), now));
    assert!(is_stale(now - chrono::Duration::minutes(20), now));
}
