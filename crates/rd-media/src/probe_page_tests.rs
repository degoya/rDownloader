//! The page address a probe reports is one a browser may open as a link (audit K9).

/// Audit K9: the page address becomes a link in the interface, so only http(s) is kept.
#[test]
fn a_page_address_that_is_no_web_page_falls_back_to_the_probed_link() {
    let probed: url::Url = "https://example.test/watch?v=1".parse().expect("url");
    for reported in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
    ] {
        let metadata: super::Metadata =
            serde_json::from_value(serde_json::json!({ "webpage_url": reported }))
                .expect("metadata");
        let candidate = super::single(metadata, &probed, "best", Default::default());
        assert_eq!(candidate.info.page_url, probed, "{reported}");
    }
    let page = super::web_page("http://example.test/page").expect("http is a web page");
    assert_eq!(page.as_str(), "http://example.test/page");
}
