//! What a probe reports for one page: an address a browser may open as a link (audit K9), and
//! a name that is more than a placeholder.

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

/// A signed-in Facebook probe names every post "Video"; the post's text, else its uploader,
/// names the file instead.
#[test]
fn a_placeholder_title_gives_way_to_the_post_text_or_the_uploader() {
    let probed: url::Url = "https://www.facebook.com/reel/1".parse().expect("url");
    let title = |value: serde_json::Value| {
        let metadata: super::Metadata = serde_json::from_value(value).expect("metadata");
        super::single(metadata, &probed, "best", Default::default())
            .info
            .title
    };
    let text = "\nSuper Smash Bros. Melee in my LIVING ROOM! \n#supersmashbros";
    assert_eq!(
        title(serde_json::json!({ "title": "Video", "description": text, "uploader": "R" })),
        "Super Smash Bros. Melee in my LIVING ROOM!"
    );
    assert_eq!(
        title(serde_json::json!({ "title": "Video", "uploader": "Hylian Tree" })),
        "Hylian Tree"
    );
    assert_eq!(title(serde_json::json!({ "title": "Video" })), "Video");
    assert_eq!(title(serde_json::json!({})), "media");
    assert_eq!(
        title(serde_json::json!({ "title": "A clip", "description": "other", "uploader": "R" })),
        "A clip"
    );
}
