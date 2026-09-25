//! Reading a YouTube video id, and reading SponsorBlock's answer about it.
//!
//! The answer is JSON, but a small and very regular piece of it: an array of objects each with
//! a `category` and a two-element `segment`. Scanning it is a few lines; pulling a JSON parser
//! into a sandboxed guest to read two numbers would be more code and more attack surface.

/// The categories worth reporting. SponsorBlock has more — `music_offtopic`, `poi_highlight`
/// and others — but these three are what somebody deciding whether to download cares about.
pub const CATEGORIES: &[&str] = &["sponsor", "selfpromo", "intro"];

/// The video id in a YouTube address, if the address is one.
///
/// Both forms people actually paste: `youtube.com/watch?v=<id>` and `youtu.be/<id>`. An id is
/// eleven characters of a known alphabet, and anything else is refused rather than sent — a
/// request built from unchecked input is how a plugin ends up asking a service about something
/// that was never a video id.
#[must_use]
pub fn video_id(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let (host, path_and_query) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let candidate = if host == "youtu.be" {
        path_and_query
            .split(['?', '&', '#'])
            .next()
            .unwrap_or_default()
            .to_owned()
    } else if host == "youtube.com" || host == "m.youtube.com" || host == "music.youtube.com" {
        let query = path_and_query.split_once('?')?.1;
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("v="))?
            .split('#')
            .next()
            .unwrap_or_default()
            .to_owned()
    } else {
        return None;
    };
    is_video_id(&candidate).then_some(candidate)
}

/// YouTube ids are eleven characters of base64url. Checking is cheap and refusing is right.
fn is_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One reported segment: what it is, and how long it lasts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub seconds: f64,
}

/// Total seconds per category in a SponsorBlock answer.
///
/// A malformed entry is skipped rather than failing the lookup: the service is not ours, its
/// answer may grow fields, and a summary that is a little short beats none at all.
#[must_use]
pub fn totals(body: &str, category: &str) -> f64 {
    let mut total = 0.0;
    let needle = format!("\"category\":\"{category}\"");
    for chunk in body.split("{") {
        if !normalised(chunk).contains(&needle) {
            continue;
        }
        if let Some(segment) = read_segment(chunk) {
            total += segment.seconds;
        }
    }
    total
}

/// The `[start,end]` pair of one object, as a duration.
fn read_segment(chunk: &str) -> Option<Segment> {
    let flat = normalised(chunk);
    let start = flat.find("\"segment\":[")? + "\"segment\":[".len();
    let end = flat[start..].find(']')? + start;
    let mut numbers = flat[start..end].split(',');
    let from: f64 = numbers.next()?.trim().parse().ok()?;
    let to: f64 = numbers.next()?.trim().parse().ok()?;
    (to > from).then_some(Segment { seconds: to - from })
}

/// The chunk without the whitespace a pretty-printed answer would carry.
fn normalised(chunk: &str) -> String {
    chunk.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A duration as a person reads it: `4m 12s`, or `38s` when there are no minutes.
#[must_use]
pub fn human_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let minutes = total / 60;
    let rest = total % 60;
    if minutes == 0 {
        format!("{rest}s")
    } else {
        format!("{minutes}m {rest}s")
    }
}

#[cfg(test)]
mod tests {
    use super::{human_duration, totals, video_id};

    #[test]
    fn both_forms_people_paste_yield_the_id() {
        assert_eq!(
            video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            video_id("https://youtu.be/dQw4w9WgXcQ?t=30").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            video_id("https://m.youtube.com/watch?list=x&v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
    }

    #[test]
    fn anything_that_is_not_a_video_id_is_refused_rather_than_asked_about() {
        // A request built from unchecked input is how a plugin ends up telling a service
        // about an address that was never a video.
        assert_eq!(video_id("https://youtube.com/watch?v=../../etc"), None);
        assert_eq!(video_id("https://youtube.com/feed/subscriptions"), None);
        assert_eq!(video_id("https://example.com/watch?v=dQw4w9WgXcQ"), None);
        assert_eq!(video_id("not a url at all"), None);
    }

    #[test]
    fn the_seconds_of_one_category_are_added_up() {
        const BODY: &str = r#"[
          {"category":"sponsor","segment":[10.0,40.0]},
          {"category":"sponsor","segment":[100.5,110.5]},
          {"category":"intro","segment":[0.0,5.0]}
        ]"#;
        assert_eq!(totals(BODY, "sponsor"), 40.0);
        assert_eq!(totals(BODY, "intro"), 5.0);
        assert_eq!(totals(BODY, "selfpromo"), 0.0);
    }

    #[test]
    fn a_malformed_entry_costs_only_itself() {
        const BODY: &str = r#"[{"category":"sponsor"},{"category":"sponsor","segment":[1,3]}]"#;
        assert_eq!(totals(BODY, "sponsor"), 2.0);
        // A segment that ends before it starts is not one.
        assert_eq!(
            totals(r#"[{"category":"sponsor","segment":[9,3]}]"#, "sponsor"),
            0.0
        );
    }

    #[test]
    fn a_duration_reads_the_way_somebody_would_say_it() {
        assert_eq!(human_duration(38.0), "38s");
        assert_eq!(human_duration(252.4), "4m 12s");
        assert_eq!(human_duration(0.0), "0s");
    }
}
