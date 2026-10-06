//! Reading a hoster's web page as text, once (RD-1120-10).
//!
//! Nitroflare and Rapidgator carried the same page primitives byte for byte, and five plugins
//! decoded entities in five slightly different ways -- four with XML's predefined set, two with
//! `&#39;` beside it, one decoding `&amp;` first and so turning `&amp;lt;` into `<`. These are
//! the primitives; the markers that say what a page means stay with each hoster. Plain Rust with
//! no dependencies, so a guest that takes it gains no import.

/// The reCAPTCHA v2 site key a page embeds: the widget's `data-sitekey` attribute first, then a
/// key carried in a `/recaptcha/` script or iframe URL ([`site_key_from_recaptcha_url`]).
#[must_use]
pub fn recaptcha_site_key(html: &str) -> Option<String> {
    const ATTRIBUTE: &str = "data-sitekey=";
    if let Some(at) = html.find(ATTRIBUTE)
        && let Some(key) =
            quoted_value(html[at + ATTRIBUTE.len()..].trim_start()).filter(|key| !key.is_empty())
    {
        return Some(key);
    }
    site_key_from_recaptcha_url(html)
}

/// `.../recaptcha/api.js?render=<key>` or `.../recaptcha/api2/anchor?...&k=<key>`. The length
/// floor rejects `render=explicit`, which is a rendering mode rather than a key.
#[must_use]
pub fn site_key_from_recaptcha_url(html: &str) -> Option<String> {
    let at = html.find("/recaptcha/")?;
    let rest = clamp(&html[at..], 400);
    ["render=", "k="].into_iter().find_map(|marker| {
        let offset = rest.find(marker)?;
        let value: String = rest[offset + marker.len()..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
            })
            .collect();
        (value.len() >= 20).then_some(value)
    })
}

/// The run of ASCII digits at the front of `text`, or `None` when there is none (or it does not
/// fit a `u64`).
#[must_use]
pub fn digits_at(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The content of a quoted attribute or JavaScript string at the front of `rest`, entities
/// decoded; `Some("")` for an explicitly empty value, `None` when `rest` does not start with a
/// quote.
#[must_use]
pub fn quoted_value(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[quote.len_utf8()..];
    let end = value.find(quote)?;
    Some(decode_entities(&value[..end]))
}

/// Text following `marker` up to the next tag, whitespace collapsed and capped at 160 bytes. A
/// `class=...` marker sits inside the open tag, so the text starts after that tag's `>`.
#[must_use]
pub fn element_text(html: &str, marker: &str) -> Option<String> {
    let at = html.find(marker)? + marker.len();
    let rest = &html[at..];
    let rest = if marker.starts_with("class=") {
        &rest[rest.find('>')? + 1..]
    } else {
        rest
    };
    let end = rest.find('<').unwrap_or(rest.len());
    let collapsed = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
    let text = clamp(&collapsed, 160).to_owned();
    (!text.is_empty()).then_some(text)
}

/// XML's five predefined entities plus `&#39;`, the apostrophe HTML escapers write; anything
/// else -- another numeric reference included -- is left as it stands, so a name cannot pick up a
/// character it was never meant to carry. `&amp;` goes last, so `&amp;lt;` stays `&lt;`.
#[must_use]
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `text` truncated to at most `max` bytes, never splitting a UTF-8 character.
#[must_use]
pub fn clamp(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::{
        clamp, decode_entities, digits_at, element_text, quoted_value, recaptcha_site_key,
    };

    #[test]
    fn entities_are_decoded_once_and_ampersand_last() {
        assert_eq!(
            decode_entities("&lt;a&gt; &quot;b&quot; &apos;c&#39; &amp;d"),
            "<a> \"b\" 'c' &d"
        );
        assert_eq!(decode_entities("&amp;lt; &amp;#39;"), "&lt; &#39;");
        assert_eq!(decode_entities("&#47; &nbsp; plain"), "&#47; &nbsp; plain");
    }

    #[test]
    fn a_quoted_value_needs_a_quote_and_keeps_an_empty_one() {
        assert_eq!(quoted_value("'a&amp;b' rest").as_deref(), Some("a&b"));
        assert_eq!(quoted_value("\"\"").as_deref(), Some(""));
        assert_eq!(quoted_value("bare"), None);
        assert_eq!(quoted_value("\"unterminated"), None);
    }

    #[test]
    fn element_text_skips_the_open_tag_of_a_class_marker() {
        let html = "<title> A  page </title><div class=\"error\">Too\n many </div>";
        assert_eq!(element_text(html, "<title>").as_deref(), Some("A page"));
        assert_eq!(
            element_text(html, "class=\"error\"").as_deref(),
            Some("Too many")
        );
        assert_eq!(element_text(html, "<h1>"), None);
    }

    #[test]
    fn clamp_never_splits_a_character_and_digits_stop_at_the_first_non_digit() {
        assert_eq!(clamp("a\u{e9}b", 2), "a");
        assert_eq!(clamp("abc", 5), "abc");
        assert_eq!(digits_at("120 seconds"), Some(120));
        assert_eq!(digits_at("x1"), None);
    }

    #[test]
    fn the_site_key_comes_from_the_widget_then_from_the_script_url() {
        let key = "fixture-site-key-of-the-widget-test";
        let widget = format!("<div class=\"g-recaptcha\" data-sitekey=\"{key}\"></div>");
        assert_eq!(recaptcha_site_key(&widget).as_deref(), Some(key));
        let script =
            format!("<script src=\"https://www.google.com/recaptcha/api.js?render={key}\">");
        assert_eq!(recaptcha_site_key(&script).as_deref(), Some(key));
        let explicit = "<script src=\"https://www.google.com/recaptcha/api.js?render=explicit\">";
        assert_eq!(recaptcha_site_key(explicit), None);
    }
}
