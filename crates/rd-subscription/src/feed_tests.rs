use super::{MAX_FEED_ITEMS, parse_date, parse_duration, parse_feed};
use url::Url;

fn base() -> Url {
    "https://example.test/feed.xml".parse().expect("base")
}

const RSS: &str = r#"<?xml version="1.0"?>
    <rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
      <channel>
        <title>The Show</title>
        <language>en-GB</language>
        <item>
          <title>Episode One</title>
          <link>https://example.test/e/1</link>
          <guid isPermaLink="false">tag:example.test,2026:1</guid>
          <pubDate>Wed, 04 Feb 2026 13:00:00 GMT</pubDate>
          <enclosure url="https://cdn.example.test/1.mp3" length="1234" type="audio/mpeg"/>
          <itunes:duration>1:02:03</itunes:duration>
          <itunes:season>2</itunes:season>
          <itunes:episode>5</itunes:episode>
        </item>
        <item>
          <title>Episode Two</title>
          <link>/e/2</link>
        </item>
      </channel>
    </rss>"#;

const ATOM: &str = r#"<?xml version="1.0"?>
    <feed xmlns="http://www.w3.org/2005/Atom">
      <title>The Blog</title>
      <entry>
        <title>First Post</title>
        <id>urn:uuid:1234</id>
        <updated>2026-02-04T13:00:00Z</updated>
        <link rel="alternate" href="https://example.test/p/1"/>
        <link rel="enclosure" href="https://cdn.example.test/1.mp4"/>
        <link rel="self" href="https://example.test/p/1.atom"/>
      </entry>
    </feed>"#;

#[test]
fn an_rss_item_yields_its_identity_address_and_podcast_metadata() {
    let feed = parse_feed(RSS, &base()).expect("feed");
    assert_eq!(feed.title, "The Show");
    assert_eq!(feed.items.len(), 2);
    let first = &feed.items[0];
    assert_eq!(first.title, "Episode One");
    assert_eq!(first.id.as_deref(), Some("tag:example.test,2026:1"));
    assert_eq!(
        first.enclosure.as_ref().map(Url::as_str),
        Some("https://cdn.example.test/1.mp3")
    );
    assert_eq!(first.duration_seconds, Some(3_723));
    assert_eq!((first.season, first.episode), (Some(2), Some(5)));
    assert_eq!(
        first.published_at.map(|value| value.to_rfc3339()),
        Some("2026-02-04T13:00:00+00:00".to_owned())
    );
}

#[test]
fn a_podcast_item_downloads_the_enclosure_not_the_show_notes() {
    // The `<link>` of a podcast item is an HTML page; downloading it instead of the
    // episode is the single most obvious way to get this wrong.
    let feed = parse_feed(RSS, &base()).expect("feed");
    assert_eq!(
        feed.items[0].download_url().map(Url::as_str),
        Some("https://cdn.example.test/1.mp3")
    );
    // An item with no enclosure falls back to its page.
    assert_eq!(
        feed.items[1].download_url().map(Url::as_str),
        Some("https://example.test/e/2")
    );
}

#[test]
fn a_relative_link_resolves_against_the_feed_address() {
    let feed = parse_feed(RSS, &base()).expect("feed");
    assert_eq!(
        feed.items[1].link.as_ref().map(Url::as_str),
        Some("https://example.test/e/2")
    );
}

#[test]
fn items_inherit_the_channel_language() {
    let feed = parse_feed(RSS, &base()).expect("feed");
    assert!(
        feed.items
            .iter()
            .all(|item| item.language.as_deref() == Some("en-GB")),
        "a language filter has to work on feeds that declare it once"
    );
}

#[test]
fn an_atom_entry_separates_its_page_from_its_enclosure() {
    let feed = parse_feed(ATOM, &base()).expect("feed");
    assert_eq!(feed.items.len(), 1);
    let entry = &feed.items[0];
    assert_eq!(entry.id.as_deref(), Some("urn:uuid:1234"));
    assert_eq!(
        entry.link.as_ref().map(Url::as_str),
        Some("https://example.test/p/1")
    );
    assert_eq!(
        entry.enclosure.as_ref().map(Url::as_str),
        Some("https://cdn.example.test/1.mp4")
    );
    // `rel="self"` is the feed's own address, not the entry's.
    assert_eq!(
        entry.download_url().map(Url::as_str),
        Some("https://cdn.example.test/1.mp4")
    );
}

#[test]
fn an_item_title_is_not_confused_with_the_feed_title() {
    let feed = parse_feed(RSS, &base()).expect("feed");
    assert_eq!(feed.title, "The Show");
    assert_eq!(feed.items[0].title, "Episode One");
}

#[test]
fn a_doctype_with_an_internal_subset_is_refused() {
    // The billion-laughs shape. Refused outright rather than trusted to the parser.
    let bomb = r#"<?xml version="1.0"?>
        <!DOCTYPE rss [<!ENTITY lol "haha"><!ENTITY lol2 "&lol;&lol;&lol;">]>
        <rss><channel><item><title>&lol2;</title></item></channel></rss>"#;
    assert!(parse_feed(bomb, &base()).is_err());
}

#[test]
fn an_external_entity_declaration_is_refused() {
    let xxe = r#"<?xml version="1.0"?>
        <!DOCTYPE rss [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
        <rss><channel><item><title>&xxe;</title></item></channel></rss>"#;
    assert!(parse_feed(xxe, &base()).is_err());
}

#[test]
fn malformed_xml_is_an_error_rather_than_a_partial_feed() {
    // A truncated document must not look like a channel that deleted its entries.
    let truncated = "<rss><channel><item><title>Half";
    assert!(parse_feed(truncated, &base()).is_err());
}

#[test]
fn a_document_that_is_not_a_feed_simply_has_no_items() {
    let html = "<html><body><p>Not a feed</p></body></html>";
    let feed = parse_feed(html, &base()).expect("parses as XML");
    assert!(feed.items.is_empty());
}

#[test]
fn cdata_and_split_text_survive_intact() {
    let body = r#"<rss><channel><item>
            <title><![CDATA[Bracketed & odd]]></title>
            <guid>x</guid>
        </item></channel></rss>"#;
    let feed = parse_feed(body, &base()).expect("feed");
    assert_eq!(feed.items[0].title, "Bracketed & odd");
}

#[test]
fn the_item_count_is_bounded() {
    let items = (0..MAX_FEED_ITEMS + 50)
        .map(|index| format!("<item><title>{index}</title><guid>{index}</guid></item>"))
        .collect::<String>();
    let body = format!("<rss><channel>{items}</channel></rss>");
    let feed = parse_feed(&body, &base()).expect("feed");
    assert_eq!(feed.items.len(), MAX_FEED_ITEMS);
}

#[test]
fn durations_are_parsed_in_all_three_shapes() {
    assert_eq!(parse_duration("3600"), Some(3_600));
    assert_eq!(parse_duration("02:03"), Some(123));
    assert_eq!(parse_duration("1:02:03"), Some(3_723));
    assert_eq!(parse_duration(""), None);
    assert_eq!(parse_duration("not a duration"), None);
}

#[test]
fn dates_are_parsed_in_both_feed_dialects() {
    assert!(parse_date("Wed, 04 Feb 2026 13:00:00 GMT").is_some());
    assert!(parse_date("2026-02-04T13:00:00Z").is_some());
    assert!(parse_date("2026-02-04T13:00:00+01:00").is_some());
    // A date nobody can read is not a reason to discard the entry.
    assert!(parse_date("last Tuesday").is_none());
}

#[test]
fn reordering_a_feed_does_not_change_what_the_items_are() {
    // Identity comes from the guid, so the order the server chose is irrelevant. The
    // archive relies on exactly this: a feed that re-sorts itself between two polls must
    // not look like a feed full of new entries.
    let item = |id: &str, title: &str| {
        format!("<item><title>{title}</title><guid>tag:example,2026:{id}</guid></item>")
    };
    let forwards = format!(
        "<rss><channel>{}{}</channel></rss>",
        item("1", "Episode One"),
        item("2", "Episode Two")
    );
    let backwards = format!(
        "<rss><channel>{}{}</channel></rss>",
        item("2", "Episode Two"),
        item("1", "Episode One")
    );

    let mut first: Vec<_> = parse_feed(&forwards, &base())
        .expect("feed")
        .items
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    let mut second: Vec<_> = parse_feed(&backwards, &base())
        .expect("feed")
        .items
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    // The documents genuinely differ in order …
    assert_ne!(first, second);
    // … but they describe the same two items.
    first.sort();
    second.sort();
    assert_eq!(first, second);
}
