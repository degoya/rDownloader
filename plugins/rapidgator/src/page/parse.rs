//! Small, purely textual primitives [`super`] is built out of: JavaScript variable
//! assignments, quoted attribute values, a minimal `<form>`/`<input>` reader, and two
//! byte-safe windowing helpers. Split out of `page.rs` to keep both files well inside the
//! crate layout's 500-line convention.
//!
//! None of these know anything about Rapidgator; the hoster-specific markers all live in
//! [`super`], which is also where they are exercised from (`page/tests.rs`).

/// `name = '<value>'` / `name = "<value>"`, skipping occurrences of `name` that are part of a
/// longer identifier or a JSON key rather than an assignment.
pub(super) fn js_string_var(html: &str, name: &str) -> Option<String> {
    quoted_value(assignment_rest(html, name)?).filter(|value| !value.is_empty())
}

/// `name = <digits>`, same skipping rules as [`js_string_var`].
pub(super) fn js_number_var(html: &str, name: &str) -> Option<u64> {
    digits_at(assignment_rest(html, name)?)
}

fn assignment_rest<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let mut cursor = html;
    loop {
        let at = cursor.find(name)?;
        let after = &cursor[at + name.len()..];
        if let Some(rest) = after.trim_start().strip_prefix('=') {
            // `==` / `=>` are comparisons, not assignments.
            let rest = rest.trim_start();
            if !rest.starts_with('=') {
                return Some(rest);
            }
        }
        cursor = after;
    }
}

/// The first run of digits after `marker`, within a short window so a number further down the
/// page cannot be mistaken for the one being looked for.
pub(super) fn number_after(html: &str, marker: &str) -> Option<u64> {
    let rest = clamp(&html[html.find(marker)? + marker.len()..], 80);
    let start = rest.find(|character: char| character.is_ascii_digit())?;
    digits_at(&rest[start..])
}

fn digits_at(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The content of a quoted attribute or JavaScript string at the front of `rest`; `Some("")` for
/// an explicitly empty value, `None` when `rest` does not start with a quote.
pub(super) fn quoted_value(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[quote.len_utf8()..];
    let end = value.find(quote)?;
    Some(decode_entities(&value[..end]))
}

/// The `<form ...>` element whose open tag carries `id="<id>"`, up to and including `</form>`.
pub(super) fn form_with_id<'a>(html: &'a str, id: &str) -> Option<&'a str> {
    let mut cursor = html;
    let mut base = 0_usize;
    loop {
        let at = [format!("id=\"{id}\""), format!("id='{id}'")]
            .iter()
            .filter_map(|needle| cursor.find(needle.as_str()))
            .min()?;
        let absolute = base + at;
        if let Some(start) = html[..absolute].rfind("<form")
            // The attribute must belong to the `<form` tag itself, not to a child element.
            && !html[start..absolute].contains('>')
        {
            let end = html[start..]
                .find("</form>")
                .map_or(html.len(), |offset| start + offset + "</form>".len());
            return Some(&html[start..end]);
        }
        base = absolute + 1;
        cursor = &html[base..];
    }
}

/// The open tag of an element, without the leading `<` and trailing `>`.
pub(super) fn open_tag(element: &str) -> &str {
    let rest = element.strip_prefix('<').unwrap_or(element);
    match rest.find('>') {
        Some(end) => &rest[..end],
        None => rest,
    }
}

/// Every `<input>` in `form_html` that carries a `name`, with its `value` (empty when absent).
pub(super) fn form_fields(form_html: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut cursor = form_html;
    while let Some(at) = cursor.find("<input") {
        let rest = &cursor[at + "<input".len()..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let Some(name) = tag_attribute(tag, "name").filter(|name| !name.is_empty()) {
            fields.push((name, tag_attribute(tag, "value").unwrap_or_default()));
        }
        cursor = &rest[end..];
    }
    fields
}

/// A quoted attribute value from an element's open tag. Unquoted values are not supported: every
/// form Rapidgator serves quotes them, and guessing where an unquoted one ends risks a silently
/// wrong field value.
pub(super) fn tag_attribute(tag: &str, name: &str) -> Option<String> {
    let mut cursor = tag;
    loop {
        let at = cursor.find(name)?;
        let after = &cursor[at + name.len()..];
        let starts_attribute = at == 0
            || cursor[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if starts_attribute
            && let Some(rest) = after.trim_start().strip_prefix('=')
            && let Some(value) = quoted_value(rest.trim_start())
        {
            return Some(value);
        }
        cursor = after;
    }
}

/// Text following `marker` up to the next tag, whitespace collapsed and capped at 160 bytes.
pub(super) fn element_text(html: &str, marker: &str) -> Option<String> {
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

/// The named HTML entities these pages use; anything else is left as it stands.
pub(super) fn decode_entities(text: &str) -> String {
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
pub(super) fn clamp(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
