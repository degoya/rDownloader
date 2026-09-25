//! Which links a brand claims, and the file id inside them.
//!
//! The shapes come from the SPA's own router (`/:id/:name.html`, `/download/free/:id`,
//! `/download/started/:id`, `/download/folder/:folderId`), from JDownloader's patterns and
//! from what `links/check` accepts. A short-domain link (`turb.pw`, `hil.to`, …) is claimed
//! and its id taken; the short domain itself is never fetched — every request goes to the main
//! site or its API. Query strings (`?short_domain=…`, `?from_mirror=1&site_version=1`) and
//! fragments are discarded.

use url::Url;

use crate::brand::Brand;

/// What a claimed link points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Link {
    /// A single file, by id.
    File(String),
    /// A folder (`/download/folder/<n>`); a crawler's business, not a resolver's.
    Folder,
}

/// Why a link is not claimed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkError {
    /// Not a URL at all.
    Invalid,
    /// A URL, but not a file or folder link of this brand.
    Unsupported,
}

/// Path segments that look like an id and are not one. JDownloader's HitFile exclusion list,
/// plus the SPA's own top-level routes, applied to both brands: a page name costs nothing to
/// refuse and a page fetched as a file is exactly the failure this plugin exists to prevent.
const RESERVED_SEGMENTS: &[&str] = &[
    "abuse",
    "api",
    "contacts",
    "download",
    "error",
    "faq",
    "favicon",
    "files",
    "impressum",
    "linkchecker",
    "locale",
    "login",
    "premium",
    "reg",
    "reseller",
    "rules",
    "rulesdownload",
    "upload",
];

/// Parses `url` into the file it names.
///
/// # Errors
///
/// [`LinkError::Invalid`] when the text is no URL, [`LinkError::Unsupported`] when the host
/// is not this brand's or the path is none of the accepted shapes.
pub fn parse(brand: &Brand, url: &str) -> Result<Link, LinkError> {
    let parsed = Url::parse(url.trim()).map_err(|_| LinkError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(LinkError::Unsupported);
    }
    let host = parsed.host_str().ok_or(LinkError::Unsupported)?;
    if !brand.claims_host(host) {
        return Err(LinkError::Unsupported);
    }
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    match segments.as_slice() {
        ["download", "free" | "started", id] => file(brand, id),
        ["download", "redirect", token, id, ..] if is_hex_token(token) => file(brand, id),
        ["download", "folder", number]
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Ok(Link::Folder)
        }
        [first] => match first.strip_suffix(".html") {
            Some(id) => file(brand, id),
            None if brand.bare_id_path => file(brand, first),
            None => Err(LinkError::Unsupported),
        },
        [id, name] if name.ends_with(".html") => file(brand, id),
        _ => Err(LinkError::Unsupported),
    }
}

fn file(brand: &Brand, id: &str) -> Result<Link, LinkError> {
    if !brand.id.accepts(id) || RESERVED_SEGMENTS.contains(&id.to_ascii_lowercase().as_str()) {
        return Err(LinkError::Unsupported);
    }
    Ok(Link::File(id.to_owned()))
}

/// The 32-hex token of an already generated `/download/redirect/<token>/<id>` link. Such a link
/// expires and is single-use, so only the id is taken from it and the file is resolved afresh.
fn is_hex_token(token: &str) -> bool {
    token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The id of `url`, when it is a file link of this brand.
#[must_use]
pub fn file_id(brand: &Brand, url: &str) -> Option<String> {
    match parse(brand, url) {
        Ok(Link::File(id)) => Some(id),
        _ => None,
    }
}
