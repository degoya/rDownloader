use url::Url;

use super::{CaptureCookie, FetchPlan, MAX_COOKIES, cookie_header, cookies_for, cookies_travel};

fn cookie(name: &str, value: &str) -> CaptureCookie {
    CaptureCookie {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn url(text: &str) -> Url {
    Url::parse(text).expect("url")
}

#[test]
fn cookies_go_to_the_handed_over_origin_and_nowhere_else() {
    let handed = url("https://indexer.test/getnzb/abc");
    assert!(cookies_travel(
        &handed,
        &url("https://indexer.test/other?x=1")
    ));
    assert!(cookies_travel(&handed, &url("https://indexer.test:443/")));
    for elsewhere in [
        "https://www.indexer.test/getnzb/abc",
        "https://cdn.indexer.test/",
        "https://indexer.test.evil.test/",
        "https://indexer.test:8443/",
        "http://indexer.test/getnzb/abc",
        "https://other.test/",
    ] {
        assert!(!cookies_travel(&handed, &url(elsewhere)), "{elsewhere}");
    }
}

#[test]
fn the_header_joins_the_cookies_and_is_marked_sensitive() {
    let header = cookie_header(&[cookie("uid", "7"), cookie("__Secure-sess", "a.b-c")])
        .expect("valid")
        .expect("some");
    assert_eq!(
        header.to_str().expect("ascii"),
        "uid=7; __Secure-sess=a.b-c"
    );
    assert!(header.is_sensitive());
    assert!(!format!("{header:?}").contains("a.b-c"));
    assert!(cookie_header(&[]).expect("none").is_none());
}

/// Nothing a caller sends may end the header or start another one.
#[test]
fn a_cookie_that_could_break_the_header_is_refused() {
    for bad in [
        cookie("", "v"),
        cookie("na me", "v"),
        cookie("name;", "v"),
        cookie("name", "v; other=1"),
        cookie("name", "v\r\nX-Injected: 1"),
        cookie("name", "caf\u{e9}"),
    ] {
        let error = cookie_header(&[bad]).expect_err("refused");
        assert_eq!(error.code(), "capture.cookies_invalid");
    }
    let many: Vec<CaptureCookie> = (0..=MAX_COOKIES)
        .map(|index| cookie(&format!("c{index}"), "v"))
        .collect();
    assert_eq!(
        cookie_header(&many).expect_err("refused").code(),
        "capture.cookies_invalid"
    );
}

#[test]
fn a_cookie_never_shows_its_value_in_debug_output() {
    let text = format!("{:?}", cookie("session", "s3cr3t-value"));
    assert!(text.contains("session"));
    assert!(!text.contains("s3cr3t-value"));
}

#[test]
fn the_plan_takes_only_web_addresses_and_clean_headers() {
    assert_eq!(
        FetchPlan::new("ftp://indexer.test/x", None, None, None)
            .err()
            .map(|error| error.code().to_owned()),
        Some("capture.url_invalid".to_owned())
    );
    assert_eq!(
        FetchPlan::new(
            "https://indexer.test/x",
            None,
            Some("javascript:alert(1)"),
            None
        )
        .err()
        .map(|error| error.code().to_owned()),
        Some("capture.header_invalid".to_owned())
    );
    assert_eq!(
        FetchPlan::new("https://indexer.test/x", None, None, Some("Agent\r\nX: 1"))
            .err()
            .map(|error| error.code().to_owned()),
        Some("capture.header_invalid".to_owned())
    );
    let plan =
        FetchPlan::new("https://indexer.test/x#part", Some(""), Some(""), Some(" ")).expect("plan");
    assert_eq!(plan.url.as_str(), "https://indexer.test/x");
    assert!(plan.referrer.is_none());
    assert!(
        plan.cookie.is_none(),
        "an empty cookie string is no cookies"
    );
    assert!(
        plan.user_agent
            .to_str()
            .expect("ascii")
            .starts_with("rDownloader/")
    );
}

/// Both spellings `capture/cookies` takes, and only what the browser would send there.
#[test]
fn the_cookies_are_the_ones_the_browser_would_send_to_that_address() {
    let cart = url("https://indexer.test/getnzb/abc");
    let file = "# Netscape HTTP Cookie File\n\
                indexer.test\tFALSE\t/\tTRUE\t0\tuid\t7\n\
                #HttpOnly_.indexer.test\tTRUE\t/getnzb\tTRUE\t1893456000\tsess\ta=b==\n";
    let cookies = cookies_for(file, &cart).expect("parsed");
    let pairs: Vec<(&str, &str)> = cookies
        .iter()
        .map(|cookie| (cookie.name.as_str(), cookie.value.as_str()))
        .collect();
    assert_eq!(pairs, [("uid", "7"), ("sess", "a=b==")]);
    let header = cookie_header(&cookies).expect("valid").expect("some");
    assert_eq!(header.to_str().expect("ascii"), "uid=7; sess=a=b==");
    let from_header = cookies_for("uid=7; sess=a=b==", &cart).expect("header");
    assert_eq!(from_header.len(), 2);
    assert!(cookies_for("  ", &cart).expect("none").is_empty());
    for outside in [
        "other.test\tFALSE\t/\tFALSE\t0\tuid\t7",
        "www.indexer.test\tFALSE\t/\tFALSE\t0\tuid\t7",
        ".test\tTRUE\t/\tFALSE\t0\tuid\t7",
        "indexer.test\tFALSE\t/account\tFALSE\t0\tuid\t7",
        "indexer.test\tFALSE\t/getnzbx\tFALSE\t0\tuid\t7",
    ] {
        let error = cookies_for(outside, &cart).expect_err(outside);
        assert_eq!(error.code(), "capture.cookies_invalid", "{outside}");
    }
    let plain = url("http://indexer.test/getnzb/abc");
    assert!(cookies_for("indexer.test\tFALSE\t/\tTRUE\t0\tuid\t7", &plain).is_err());
    assert!(super::path_matches("/getnzb", "/getnzb/abc"));
    assert!(super::path_matches("/", "/getnzb/abc"));
    assert!(!super::path_matches("/getnzb", "/getnzbx"));
}

/// Nothing that could end the header or start another one gets as far as the wire.
#[test]
fn a_cookie_string_that_could_break_the_header_is_refused() {
    let cart = url("https://indexer.test/getnzb/abc");
    for bad in ["novalue", "uid=7\r\nX-Injected: 1"] {
        let refused = cookies_for(bad, &cart).and_then(|cookies| cookie_header(&cookies));
        assert_eq!(
            refused.expect_err(bad).code(),
            "capture.cookies_invalid",
            "{bad}"
        );
    }
    let long = format!("a={}", "b".repeat(super::MAX_COOKIE_TEXT_BYTES));
    assert_eq!(
        cookies_for(&long, &cart).expect_err("long").code(),
        "capture.cookies_invalid"
    );
}

/// Finding 9 of the 2026-09-28 security review: `fetch_once` asks this before every hop,
/// the first and each redirect alike, so a hop is what is pinned here.
#[tokio::test]
async fn a_hop_to_this_machine_or_the_metadata_endpoint_is_refused() {
    let policy = rd_http::AddressPolicy::new(true);
    for target in [
        "http://127.0.0.1:8710/api/v1/settings",
        "http://[::1]:8710/",
        "http://169.254.169.254/latest/meta-data/",
        "http://0.0.0.0:8710/",
        "http://localhost:8710/api/v1/settings",
    ] {
        let refused = super::screen_hop(&policy, &Url::parse(target).expect("url")).await;
        assert_eq!(
            refused
                .err()
                .map(|error| error.code().to_owned())
                .as_deref(),
            Some("capture.fetch_address_refused"),
            "{target}"
        );
    }
    for target in [
        "http://192.168.1.10/getnzb/abc",
        "https://93.184.215.14/file.nzb",
    ] {
        assert!(
            super::screen_hop(&policy, &Url::parse(target).expect("url"))
                .await
                .is_ok(),
            "{target}"
        );
    }
}
