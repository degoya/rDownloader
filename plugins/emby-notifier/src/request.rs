//! The one request a finished package becomes: Emby is told the package's folder is new, or scans
//! every library when it cannot be told which folder.
//!
//! Since contract `0.11.0` (RD-1250-01) the notification names the finished package's folder.
//! When the target's mapping holds it, Emby is told that folder was created
//! (`POST /Library/Media/Updated`) and rescans the folder that holds it, not every library
//! (Emby 4.9.1.80, recorded 2026-10-11). Without a folder, or with one the mapping does not hold,
//! it scans every library (`POST /Library/Refresh`), as before. Either way Emby queues the
//! work as a task and answers at once.

use plugin_guest_notifier::media_path;

/// What Emby is asked without a folder: scan every library.
pub const REFRESH_PATH: &str = "/Library/Refresh";

/// What Emby is told with a folder: these paths changed.
pub const UPDATED_PATH: &str = "/Library/Media/Updated";

/// The setting mapping rDownloader's folders onto the server's, one `service=server` per line.
pub const MAPPING_SETTING: &str = "path_mapping";

/// The `Authorization` value Emby reads its API key from. Emby also takes `X-Emby-Token` or an
/// `api_key` query, but `Authorization` is the only one of those names the host lets a plugin set
/// (RD-120-60), and a key in a header stays out of every address a log might print. The host
/// substitutes the marker; the plugin never sees the key.
pub const AUTHORIZATION: &str = "MediaBrowser Token=\"{{secret}}\"";

/// Whether this event is one a library refresh is for. Only a finished package adds files to
/// the library; a rule that also sends this target other events gets no refresh for them.
#[must_use]
pub fn wanted(event: &str) -> bool {
    event == "package_completed"
}

/// The folder Emby is told about: the package's, as the server sees it, when the mapping
/// holds it.
#[must_use]
pub fn folder(mapping: &str, package_path: Option<&str>) -> Option<String> {
    media_path::mapped(mapping, package_path?)
}

/// The body that tells Emby `folder` was created: its `MediaUpdateInfoDto`, one update.
#[must_use]
pub fn updated_body(folder: &str) -> String {
    format!(
        r#"{{"Updates":[{{"Path":{},"UpdateType":"Created"}}]}}"#,
        media_path::json_string(folder)
    )
}

/// The address to send the request to -- the update when there is a folder to name, else the
/// refresh -- or `None` when the destination is not the server's address.
///
/// The destination is the server as the person reaches it -- `http://192.168.1.10:8096`, or
/// behind a proxy with a base path of its own. Whether it is an address is read exactly as the
/// host reads it, which narrows the delivery to that address's host (RD-130-15); a query or
/// fragment someone pasted along is dropped, since the refresh is a request of its own.
#[must_use]
pub fn endpoint(destination: &str, with_folder: bool) -> Option<String> {
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
    let path = if with_folder {
        UPDATED_PATH
    } else {
        REFRESH_PATH
    };
    Some(format!("{server}{path}"))
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
