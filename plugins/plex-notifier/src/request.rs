//! The one request a finished package becomes: Plex scans the package's folder in one library
//! section, or every section when it cannot be told which folder.
//!
//! Since contract `0.11.0` (RD-1250-01) the notification names the finished package's folder.
//! Plex scans a folder only within a section (`/library/sections/<id>/refresh?path=<folder>`),
//! and which section holds it is not asked of the server -- the plugin reads nothing of the
//! library -- so the person names it in the `section` setting. A section and a folder the
//! mapping holds: that folder is scanned. A section alone: that section. Neither: every
//! section, as before. Plex starts the scan in the background and answers at once, so a refresh
//! per finished package costs the server a scan, not a request that waits for one.

use plugin_guest_notifier::media_path;

/// Plex's own spelling for "every section".
pub const ALL_SECTIONS: &str = "all";

/// The setting naming the library section to scan; unset, every section is.
pub const SECTION_SETTING: &str = "section";

/// The setting mapping rDownloader's folders onto the server's, one `service=server` per line.
pub const MAPPING_SETTING: &str = "path_mapping";

/// The query name Plex reads its token from. The host allows no header of that name
/// (RD-120-60), and Plex reads the query as readily.
pub const TOKEN_QUERY: &str = "X-Plex-Token";

/// The query name Plex reads the folder to scan from.
pub const PATH_QUERY: &str = "path";

/// Whether this event is one a library refresh is for. Only a finished package adds files to
/// the library; a rule that also sends this target other events gets no refresh for them.
#[must_use]
pub fn wanted(event: &str) -> bool {
    event == "package_completed"
}

/// The section the target names, `None` when it names none, or the reason it is no section:
/// Plex numbers its sections, and anything else would be answered with a `404` on every
/// finished package.
///
/// # Errors
///
/// When the setting holds anything but digits.
pub fn section(setting: Option<&str>) -> Result<Option<&str>, &'static str> {
    match setting.map(str::trim) {
        None | Some("") => Ok(None),
        Some(id) if id.bytes().all(|byte| byte.is_ascii_digit()) => Ok(Some(id)),
        Some(_) => Err("The library section of this destination is not a number"),
    }
}

/// The folder Plex is asked to scan: the package's, as the server sees it, when a section is
/// named and the mapping holds the folder; else none, and the section is scanned whole.
#[must_use]
pub fn folder(section: Option<&str>, mapping: &str, package_path: Option<&str>) -> Option<String> {
    section?;
    media_path::mapped(mapping, package_path?)
}

/// The address to send the refresh to, or `None` when the destination is not the server's
/// address.
///
/// The destination is the server as the person reaches it -- `http://192.168.1.10:32400`, or
/// behind a proxy with a path of its own. Whether it is an address is read exactly as the host
/// reads it, which narrows the delivery to that address's host (RD-130-15); a query or fragment
/// someone pasted along is dropped, since the refresh is a request of its own.
#[must_use]
pub fn endpoint(destination: &str, section: Option<&str>) -> Option<String> {
    let destination = destination.trim();
    let written_as_address = ["https://", "http://"].iter().any(|scheme| {
        destination
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    if !written_as_address {
        return None;
    }
    let server = destination
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    let section = section.unwrap_or(ALL_SECTIONS);
    Some(format!("{server}/library/sections/{section}/refresh"))
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
