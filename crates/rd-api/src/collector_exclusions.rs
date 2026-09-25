//! Domain blocklist dropping links at LinkGrabber intake.
//!
//! The list is a user-editable text file so hosts can be blocked without a restart. Intake
//! would otherwise read it on every request, hence the path + modification-time cache.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::SystemTime,
};

use crate::ApiError;

/// Same cap as the password list: a pasted-in file must not grow the matcher unbounded.
const MAX_ENTRIES: usize = 10_000;

struct Cached {
    path: PathBuf,
    /// `None` while the file is missing or its metadata unreadable; a readable timestamp
    /// later on then invalidates this entry.
    modified: Option<SystemTime>,
    entries: Arc<Vec<String>>,
}

/// Only ever held around a file read, never across an await.
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Blocklist configured in the service settings, empty when none applies.
pub async fn blocklist(database: &rd_db::Database) -> Result<Arc<Vec<String>>, ApiError> {
    let configured = database
        .service_setting_field::<String>("excluded_domains_file")
        .await?;
    Ok(excluded_domains(configured.as_deref()))
}

/// Default file name looked up next to the database when no path is configured.
const DEFAULT_FILE_NAME: &str = "excluded_domains.txt";

/// An unset setting falls back to `excluded_domains.txt` next to the database, mirroring how
/// the password list is resolved. Without a registered data directory (tests, tools) there is
/// no blocklist.
#[must_use]
pub fn excluded_domains(setting: Option<&str>) -> Arc<Vec<String>> {
    let configured = setting
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let Some(path) =
        configured.or_else(|| rd_core::data_directory().map(|dir| dir.join(DEFAULT_FILE_NAME)))
    else {
        return Arc::new(Vec::new());
    };
    let path = path.as_path();
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok();
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(cached) = cache.as_ref()
        && cached.path == path
        && cached.modified == modified
    {
        return Arc::clone(&cached.entries);
    }
    let entries = Arc::new(load_excluded_domains(path));
    *cache = Some(Cached {
        path: path.to_path_buf(),
        modified,
        entries: Arc::clone(&entries),
    });
    entries
}

/// A missing file yields an empty list; a blocklist nobody wrote blocks nothing.
#[must_use]
pub fn load_excluded_domains(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).map_or_else(|_| Vec::new(), |content| parse(&content))
}

/// One host per line, `#` comments and blank lines ignored.
#[must_use]
pub fn parse(content: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(normalize)
        .filter(|entry| seen.insert(entry.clone()))
        .take(MAX_ENTRIES)
        .collect()
}

/// Accepts what people actually paste: a bare host, a `www.` prefixed one, or a full URL.
fn normalize(value: &str) -> Option<String> {
    let after_scheme = value.split_once("://").map_or(value, |(_, rest)| rest);
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme)
        .trim()
        .to_ascii_lowercase();
    let host = host.trim_start_matches("www.");
    (!host.is_empty()).then(|| host.to_owned())
}

/// Same semantics as `rd_core::MediaSettings::handles_host`: a host matches an entry exactly
/// or as one of its subdomains.
#[must_use]
pub fn is_excluded(entries: &[String], host: &str) -> bool {
    if entries.is_empty() {
        return false;
    }
    let host = host.trim().to_ascii_lowercase();
    let host = host.trim_start_matches("www.");
    entries
        .iter()
        .any(|entry| host == entry.as_str() || host.ends_with(&format!(".{entry}")))
}

#[cfg(test)]
mod tests {
    use super::{is_excluded, load_excluded_domains, parse};

    #[test]
    fn comments_blanks_and_duplicates_are_dropped() {
        let entries = parse("# header\n\n  example.com  \nexample.com\n#trailing\nother.net\n");
        assert_eq!(entries, ["example.com", "other.net"]);
    }

    #[test]
    fn pasted_urls_and_www_are_reduced_to_the_host() {
        assert_eq!(
            parse("https://www.Example.COM/some/path?q=1\nhttp://cdn.example.org\n"),
            ["example.com", "cdn.example.org"]
        );
    }

    #[test]
    fn exact_and_subdomain_hosts_match() {
        let entries = parse("example.com\n");
        assert!(is_excluded(&entries, "example.com"));
        assert!(is_excluded(&entries, "www.EXAMPLE.com"));
        assert!(is_excluded(&entries, "files.cdn.example.com"));
    }

    #[test]
    fn unrelated_hosts_do_not_match() {
        let entries = parse("example.com\n");
        assert!(!is_excluded(&entries, "notexample.com"));
        assert!(!is_excluded(&entries, "example.com.evil.net"));
        assert!(!is_excluded(&entries, "example.org"));
        assert!(!is_excluded(&[], "example.com"));
    }

    #[test]
    fn missing_file_yields_an_empty_list() {
        let path = std::env::temp_dir().join("rd-excluded-domains-does-not-exist.txt");
        assert!(load_excluded_domains(&path).is_empty());
    }
}
