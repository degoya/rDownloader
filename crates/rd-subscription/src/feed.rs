//! RSS, Atom and podcast parsing (RD-080-10).
//!
//! Pure parsing over a string, so every case below — a malformed document, a feed that
//! reorders itself, an entry with no id — is a fixture rather than a network.
//!
//! **A feed is hostile input.** It is XML fetched from a third party, so the parser refuses
//! a DOCTYPE with an internal subset or an entity declaration rather than trusting that
//! `quick_xml` will not expand one; that is the same rule the NZB and WebDAV parsers apply,
//! and it is what keeps a billion-laughs document from being a denial of service.
//!
//! What this module deliberately does *not* do is decide identity or novelty. Those belong
//! to the subscription core, which applies them the same way to every source kind.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use quick_xml::{Reader, events::Event};
use url::Url;

/// Largest feed document accepted.
pub const MAX_FEED_BYTES: usize = 16 * 1024 * 1024;
/// Most entries taken from one document.
pub const MAX_FEED_ITEMS: usize = 1_000;

/// One entry of a feed, before identity and filtering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedItem {
    /// `<guid>` or Atom `<id>`; the strongest identity a feed offers.
    pub id: Option<String>,
    pub title: String,
    /// The item's page.
    pub link: Option<Url>,
    /// The downloadable file, when the feed names one. For a podcast this is the episode.
    pub enclosure: Option<Url>,
    /// The media type the feed declares for that file.
    ///
    /// An indexer says outright what it is handing over — `application/x-nzb` from Newznab,
    /// `application/x-bittorrent` from Torznab — and that is better evidence than anything
    /// derived from the address, which for these is an API call with no telling extension.
    pub enclosure_type: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    /// The publication date exactly as written, so an unparseable one still contributes to
    /// identity instead of collapsing every undated item into one.
    pub published_raw: Option<String>,
    pub duration_seconds: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub language: Option<String>,
    /// Byte size, from `<enclosure length>` or a Newznab/Torznab `size` attribute.
    pub size_bytes: Option<u64>,
    /// Categories the source assigned, as it named them.
    pub categories: Vec<String>,
    /// `<newznab:attr>` / `<torznab:attr>` name/value pairs (RD-080-11), lowercased names.
    ///
    /// Kept as a map rather than typed fields because indexers disagree about which
    /// attributes they emit and inventing an enum would only mean discarding the rest.
    pub attributes: std::collections::BTreeMap<String, String>,
}

impl FeedItem {
    /// The address to hand to the queue: the enclosure if there is one, else the page.
    ///
    /// A podcast item's `<link>` is a show-notes page, so preferring the enclosure is what
    /// makes a podcast subscription download the episode rather than the HTML around it.
    #[must_use]
    pub fn download_url(&self) -> Option<&Url> {
        self.enclosure.as_ref().or(self.link.as_ref())
    }
}

/// A parsed feed.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub title: String,
    /// Feed-level language, inherited by items that do not state their own.
    pub language: Option<String>,
    pub items: Vec<FeedItem>,
}

/// Parses an RSS 2.0 or Atom document.
///
/// The two formats are handled by one pass rather than sniffed first: they disagree on
/// element names but not on structure, and a single pass cannot be fooled by a document that
/// looks like one and continues as the other.
pub fn parse_feed(body: &str, base: &Url) -> Result<Feed> {
    if body.len() > MAX_FEED_BYTES {
        bail!("feed exceeds the size limit");
    }
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);

    let mut feed = Feed::default();
    let mut item: Option<FeedItem> = None;
    // The element path, so `<title>` inside an item is not mistaken for the feed's own.
    let mut path: Vec<String> = Vec::new();
    let mut in_item = false;
    let mut text = String::new();

    loop {
        match reader.read_event()? {
            Event::Eof => {
                // An unclosed element means the document was truncated. `quick_xml` does not
                // treat that as an error, and a partial feed is dangerous here: half a
                // document is indistinguishable from a channel that deleted its entries.
                if !path.is_empty() {
                    bail!("feed document ended inside <{}>", path.join("/"));
                }
                break;
            }
            Event::DocType(doctype) => validate_doctype(&doctype.decode()?)?,
            Event::Start(element) => {
                let name = local_name(element.name().as_ref());
                if is_item_element(&name) {
                    in_item = true;
                    item = Some(FeedItem::default());
                } else if in_item {
                    apply_item_attributes(item.as_mut(), &name, &element, base);
                }
                path.push(name);
                text.clear();
            }
            Event::Empty(element) => {
                let name = local_name(element.name().as_ref());
                if in_item {
                    apply_item_attributes(item.as_mut(), &name, &element, base);
                }
            }
            Event::Text(value) => {
                // Appended rather than assigned: a value split by an entity reference arrives
                // as several text events and would otherwise be truncated at the first one.
                text.push_str(&value.decode()?);
            }
            Event::CData(value) => {
                text.push_str(&String::from_utf8_lossy(value.as_ref()));
            }
            Event::End(element) => {
                let name = local_name(element.name().as_ref());
                if is_item_element(&name) {
                    if let Some(finished) = item.take()
                        && feed.items.len() < MAX_FEED_ITEMS
                    {
                        feed.items.push(finished);
                    }
                    in_item = false;
                } else if in_item {
                    apply_item_text(item.as_mut(), &name, text.trim(), base);
                } else {
                    apply_feed_text(&mut feed, &name, text.trim());
                }
                path.pop();
                text.clear();
            }
            _ => {}
        }
    }

    // An item with no language of its own inherits the channel's, which is what makes a
    // language filter work on the feeds that only declare it once.
    if let Some(language) = feed.language.clone() {
        for item in &mut feed.items {
            item.language.get_or_insert(language.clone());
        }
    }
    Ok(feed)
}

/// Refuses anything but a bare external doctype.
///
/// An internal subset is where entity declarations live, and an entity declaration is how a
/// small document becomes a large one or reads a local file.
fn validate_doctype(value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.contains(['[', ']']) || trimmed.to_ascii_lowercase().contains("<!entity") {
        bail!("feed doctype declares an internal subset");
    }
    Ok(())
}

fn is_item_element(name: &str) -> bool {
    name == "item" || name == "entry"
}

fn local_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    // The prefix matters for `itunes:duration`, so it is kept rather than stripped; only the
    // namespace *declaration* is irrelevant here.
    name.to_ascii_lowercase()
}

fn apply_feed_text(feed: &mut Feed, name: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    match name {
        "title" if feed.title.is_empty() => feed.title = text.to_owned(),
        "language" | "dc:language" if feed.language.is_none() => {
            feed.language = Some(text.to_owned());
        }
        _ => {}
    }
}

fn apply_item_text(item: Option<&mut FeedItem>, name: &str, text: &str, base: &Url) {
    let Some(item) = item else { return };
    if text.is_empty() {
        return;
    }
    match name {
        "title" if item.title.is_empty() => item.title = text.to_owned(),
        "guid" | "id" if item.id.is_none() => item.id = Some(text.to_owned()),
        // RSS puts the address in the element body; Atom puts it in a `href` attribute, which
        // `apply_item_attributes` has already handled.
        "link" if item.link.is_none() => item.link = base.join(text).ok(),
        "pubdate" | "published" | "updated" | "dc:date" if item.published_at.is_none() => {
            item.published_raw = Some(text.to_owned());
            item.published_at = parse_date(text);
        }
        "itunes:duration" if item.duration_seconds.is_none() => {
            item.duration_seconds = parse_duration(text);
        }
        "itunes:season" if item.season.is_none() => item.season = text.parse().ok(),
        "itunes:episode" if item.episode.is_none() => item.episode = text.parse().ok(),
        "language" | "dc:language" if item.language.is_none() => {
            item.language = Some(text.to_owned());
        }
        "category" => item.categories.push(text.to_owned()),
        _ => {}
    }
}

fn apply_item_attributes(
    item: Option<&mut FeedItem>,
    name: &str,
    element: &quick_xml::events::BytesStart<'_>,
    base: &Url,
) {
    let Some(item) = item else { return };
    let attribute = |key: &str| -> Option<String> {
        element
            .attributes()
            .filter_map(Result::ok)
            .find(|value| local_name(value.key.as_ref()) == key)
            // Unescaped, not read raw: every indexer's download address separates its query
            // parameters with `&`, which XML writes as `&amp;`. Kept literally it produces a
            // parameter called `amp;id` and a request the server does not recognise.
            .map(|value| {
                value
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map_or_else(
                        |_| String::from_utf8_lossy(value.value.as_ref()).into_owned(),
                        |unescaped| unescaped.into_owned(),
                    )
            })
    };
    match name {
        "enclosure" => {
            if let Some(url) = attribute("url").and_then(|value| base.join(&value).ok()) {
                item.enclosure.get_or_insert(url);
                if let Some(kind) = attribute("type") {
                    let kind = kind.trim().to_ascii_lowercase();
                    if !kind.is_empty() {
                        item.enclosure_type.get_or_insert(kind);
                    }
                }
            }
            if let Some(length) = attribute("length").and_then(|value| value.parse().ok())
                && length > 0
            {
                item.size_bytes.get_or_insert(length);
            }
        }
        // Newznab and Torznab hang their metadata off repeated `<attr>` elements rather
        // than inventing elements, so one arm handles both.
        "newznab:attr" | "torznab:attr" => {
            if let (Some(key), Some(value)) = (attribute("name"), attribute("value")) {
                let key = key.to_ascii_lowercase();
                if key == "size"
                    && let Ok(size) = value.parse::<u64>()
                    && size > 0
                {
                    item.size_bytes.get_or_insert(size);
                }
                if key == "category" {
                    item.categories.push(value.clone());
                }
                item.attributes.insert(key, value);
            }
        }
        "link" => {
            // Atom. `rel="enclosure"` is the downloadable file; `alternate` (or no rel at
            // all, which means alternate) is the page.
            let Some(href) = attribute("href").and_then(|value| base.join(&value).ok()) else {
                return;
            };
            match attribute("rel").as_deref() {
                Some("enclosure") => {
                    item.enclosure.get_or_insert(href);
                }
                Some("alternate") | None => {
                    item.link.get_or_insert(href);
                }
                // `self`, `related`, `via` and the rest are not the item.
                _ => {}
            }
        }
        "media:content" => {
            if let Some(url) = attribute("url").and_then(|value| base.join(&value).ok()) {
                item.enclosure.get_or_insert(url);
            }
        }
        _ => {}
    }
}

/// Parses the date formats feeds actually use.
///
/// RSS says RFC 2822 and Atom says RFC 3339, but plenty of feeds write neither exactly, so
/// both are tried and a failure is `None` rather than an error — a bad date is not a reason
/// to discard an entry, only a reason not to place it in time.
#[must_use]
pub fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if let Ok(parsed) = DateTime::parse_from_rfc2822(trimmed) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(parsed.with_timezone(&Utc));
    }
    // Some feeds write a naive local timestamp; treated as UTC, which is at worst a few
    // hours out and never silently reorders a feed.
    chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Parses `<itunes:duration>`, which is seconds, `MM:SS`, or `HH:MM:SS`.
#[must_use]
pub fn parse_duration(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains(':') {
        return trimmed.parse().ok();
    }
    let mut seconds: u32 = 0;
    for part in trimmed.split(':') {
        let component: u32 = part.trim().parse().ok()?;
        seconds = seconds.checked_mul(60)?.checked_add(component)?;
    }
    Some(seconds)
}

#[cfg(test)]
mod escaping_tests {
    use super::parse_feed;
    use url::Url;

    #[test]
    fn an_escaped_ampersand_in_an_address_is_decoded() {
        // Every indexer's download address separates parameters with `&`, which XML writes as
        // `&amp;`. Read literally it yields a parameter called `amp;id`, the server does not
        // recognise the request, and the answer is a short error body rather than the file.
        let document = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><item>
  <title>Example</title>
  <enclosure url="https://indexer.test/api?t=get&amp;id=abc&amp;apikey=K" type="application/x-nzb"/>
</item></channel></rss>"#;

        let feed = parse_feed(
            document,
            &Url::parse("https://indexer.test/api").expect("base"),
        )
        .expect("parse");

        let item = feed.items.first().expect("one item");
        assert_eq!(
            item.enclosure.as_ref().map(Url::as_str),
            Some("https://indexer.test/api?t=get&id=abc&apikey=K")
        );
        assert_eq!(item.enclosure_type.as_deref(), Some("application/x-nzb"));
    }
}

#[cfg(test)]
mod tests {
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
}
