//! The finished package's folder as a media server sees it (RD-1250-01).
//!
//! rDownloader and the media server rarely see a folder under the same name: one runs in a
//! container with `/downloads`, the other mounts the same disk as `/data/media`, or one is a
//! Windows machine with `D:\Downloads`. The person writes the mapping as the target's setting,
//! one `service path=server path` per line, and the longest service path that holds the folder
//! decides. A folder no line holds, or none at all, is no folder to name: the plugin falls back
//! to refreshing the whole library, which is what it did before it was told a path.

/// What the host puts in place of every `{` of text somebody else wrote (`foreign_text.rs` in
/// the host). A folder that carried one is no longer the folder on disk, so it is never named.
const INERT_BRACE: char = '\u{2774}';

/// The folder `path` (the service's) is on the media server, by `mapping`, or `None` when no
/// line of the mapping holds it.
///
/// A line is `service path=server path`, both trimmed; a line without `=`, or with nothing
/// before it, is no mapping. A service path holds a folder when the folder is that path or lies
/// beneath it, segment by segment -- `/srv/media` holds `/srv/media/Film` and not
/// `/srv/media2`. Of the lines that hold it, the longest service path wins, so a narrower line
/// can send one folder elsewhere (of two equally long, the later line). A Windows path -- a
/// drive letter, or a backslash in it -- is compared ignoring case, as Windows does. What lies
/// below the service path is appended to the server path in the server path's own separator.
#[must_use]
pub fn mapped(mapping: &str, path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.contains(INERT_BRACE) {
        return None;
    }
    let (_, server, rest) = mapping
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(service, server)| (trimmed(service), server.trim()))
        .filter(|(service, server)| !service.is_empty() && !server.is_empty())
        .filter_map(|(service, server)| below(path, service).map(|rest| (service, server, rest)))
        .max_by_key(|(service, _, _)| service.len())?;
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        return Some(server.to_owned());
    }
    let separator = if server.contains('\\') && !server.contains('/') {
        '\\'
    } else {
        '/'
    };
    let rest: String = rest
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' {
                separator
            } else {
                character
            }
        })
        .collect();
    let server = server.trim_end_matches(['/', '\\']);
    Some(format!("{server}{separator}{rest}"))
}

/// A service path without the separators it may end in, except a root's own.
fn trimmed(service: &str) -> &str {
    let service = service.trim();
    let bare = service.trim_end_matches(['/', '\\']);
    if bare.is_empty() { service } else { bare }
}

/// What of `path` lies below `service`, or `None` when `service` does not hold it.
fn below<'a>(path: &'a str, service: &str) -> Option<&'a str> {
    let head = path.get(..service.len())?;
    let same = if windows_path(service) {
        head.eq_ignore_ascii_case(service)
    } else {
        head == service
    };
    if !same {
        return None;
    }
    let rest = &path[service.len()..];
    let at_boundary =
        rest.is_empty() || rest.starts_with(['/', '\\']) || service.ends_with(['/', '\\']);
    at_boundary.then_some(rest)
}

/// Whether a path is written the Windows way: a drive letter, or a backslash anywhere.
fn windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.contains('\\') || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// `text` as a JSON string literal, quotes included: what a media server's update request
/// carries a folder in.
#[must_use]
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                out.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "media_path_tests.rs"]
mod tests;
