//! Server paths as the FTP/FTPS, SFTP and WebDAV sources split them (RD-1120-12): always
//! `/`-separated, whatever the platform the service runs on.

/// Collapses a trailing slash, and maps an empty path to the server root.
#[must_use]
pub fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    trimmed.to_owned()
}

/// The last non-empty segment of `path`; empty for the root.
#[must_use]
pub fn file_name(path: &str) -> String {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// The folder `path` sits in; the root for an entry directly under it.
#[must_use]
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => path[..index].to_owned(),
    }
}
