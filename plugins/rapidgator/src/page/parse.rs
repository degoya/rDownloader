//! Small, purely textual primitives [`super`] is built out of: JavaScript variable
//! assignments and a minimal `<form>`/`<input>` reader, on top of `plugin_common::html`'s quoted
//! values and byte-safe windows. Split out of `page.rs` to keep both files well inside the crate
//! layout's 500-line convention.
//!
//! None of these know anything about Rapidgator; the hoster-specific markers all live in
//! [`super`], which is also where they are exercised from (`page/tests.rs`).

use plugin_common::html::{clamp, digits_at, quoted_value};

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
