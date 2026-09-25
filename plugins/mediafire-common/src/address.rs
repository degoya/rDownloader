//! Reading MediaFire addresses and building the ones the plugins hand out.
//!
//! The forms are the ones measured live on 2026-09-21 (`docs/roadmap/jobs/103-06-mediafire.md`,
//! section 5) plus the ones JDownloader's `MediafireComFolder` decrypter still recognises:
//! `/file/<key>[/<name>[/file]]`, `/file_premium/<key>`, `/download/<key>`, `/view/<key>`,
//! `/listen/<key>`, `/watch/<key>`, `/download.php?<key>` (a file), `/?<key>`, `/?<key>,<key>`,
//! `/folder/<key>[/<name>|/shared]` and the `mfi.re` short host. A key is 11 to 15 lower-case
//! letters and digits; a file key and a folder key cannot be told apart by their shape, only
//! by the path they sit in — so a bare `/?<key>` is [`Address::Bare`], and whoever reads it
//! has to ask the service which of the two it is.

use url::Url;

/// Hosts the plugins claim. `app.mediafire.com` serves the same paths behind an application
/// shell; `mfi.re` redirects to `www.mediafire.com`.
pub const HOSTS: &[&str] = &[
    "mediafire.com",
    "www.mediafire.com",
    "app.mediafire.com",
    "mfi.re",
    "www.mfi.re",
];

/// Where every request goes: the API and the file page both live here.
pub const PRIMARY_HOST: &str = "www.mediafire.com";

/// Path segments that name a single file.
const FILE_SEGMENTS: &[&str] = &[
    "file",
    "file_premium",
    "download",
    "view",
    "listen",
    "watch",
];

/// Shortest and longest key seen live or in the API documentation.
const KEY_LENGTH: std::ops::RangeInclusive<usize> = 11..=15;

/// Most keys one `/?a,b,c` address may carry — the API takes up to 500 per call; a pasted
/// list beyond this is not a share link but a scrape.
pub const MAX_LISTED_KEYS: usize = 100;

/// What an address points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Address {
    /// One file, by its quick key.
    File { key: String },
    /// One folder, by its folder key.
    Folder { key: String },
    /// A key without a path: a file or a folder, and only the service knows which.
    Bare { key: String },
    /// Several file keys in one address, `/?key,key`.
    Keys(Vec<String>),
}

/// Whether `value` has the shape of a MediaFire key.
#[must_use]
pub fn is_key(value: &str) -> bool {
    KEY_LENGTH.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Reads an address, or `None` for anything that is not one of MediaFire's.
#[must_use]
pub fn parse(url: &str) -> Option<Address> {
    let parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.username().is_empty() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if !HOSTS.contains(&host.as_str()) {
        return None;
    }
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    match segments.as_slice() {
        [] => from_query(parsed.query()?),
        // `download.php?<key>` names one file — the site redirects it to `/file/<key>` — so
        // it is decided here, unlike the bare `/?<key>`; a list there was never a form.
        ["download.php"] => match from_query(parsed.query()?)? {
            Address::Bare { key } => Some(Address::File { key }),
            _ => None,
        },
        ["folder", key, ..] if is_key(key) => Some(Address::Folder {
            key: (*key).to_owned(),
        }),
        [kind, key, ..] if FILE_SEGMENTS.contains(kind) && is_key(key) => Some(Address::File {
            key: (*key).to_owned(),
        }),
        // The application shell serves `app.mediafire.com/<key>` for a file or a folder.
        [key] if host == "app.mediafire.com" && is_key(key) => Some(Address::Bare {
            key: (*key).to_owned(),
        }),
        _ => None,
    }
}

/// `?<key>` and `?<key>,<key>`: the whole query is the key list, there are no parameters.
fn from_query(query: &str) -> Option<Address> {
    let keys: Vec<&str> = query.split(',').collect();
    if keys.len() > MAX_LISTED_KEYS || !keys.iter().all(|key| is_key(key)) {
        return None;
    }
    match keys.as_slice() {
        [key] => Some(Address::Bare {
            key: (*key).to_owned(),
        }),
        _ => Some(Address::Keys(keys.into_iter().map(str::to_owned).collect())),
    }
}

/// The page that carries a file's download button.
#[must_use]
pub fn file_page(key: &str) -> String {
    format!("https://{PRIMARY_HOST}/file/{key}")
}

/// The canonical address of one file, as the API's `normal_download` spells it. `name` is
/// the file name the API reported; the address works without it.
#[must_use]
pub fn file_link(key: &str, name: Option<&str>) -> String {
    match name.filter(|name| !name.is_empty()) {
        Some(name) => format!(
            "https://{PRIMARY_HOST}/file/{key}/{}/file",
            percent_encode(name)
        ),
        None => file_page(key),
    }
}

/// One API call's address, `response_format=json` included.
#[must_use]
pub fn api_call(call: &str) -> String {
    format!("https://{PRIMARY_HOST}/api/1.5/{call}.php")
}

/// Whether `host` is one of the delivery hosts a direct link points at:
/// `download<digits>.mediafire.com` or `download<digits>.mediafirecdn.com`.
#[must_use]
pub fn is_download_host(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("download") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && matches!(&rest[digits..], ".mediafire.com" | ".mediafirecdn.com")
}

/// The `errno` of an `error.php?errno=<n>` address, which is where the site redirects a
/// page request it will not answer.
#[must_use]
pub fn error_number(url: &str) -> Option<u32> {
    let parsed = Url::parse(url).ok()?;
    if !parsed.path().ends_with("/error.php") {
        return None;
    }
    parsed
        .query_pairs()
        .find(|(name, _)| name == "errno")
        .and_then(|(_, value)| value.parse().ok())
}

/// Percent-encodes one path segment, leaving the characters a file name commonly carries.
fn percent_encode(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{Address, error_number, file_link, is_download_host, parse};

    fn file(key: &str) -> Option<Address> {
        Some(Address::File {
            key: key.to_owned(),
        })
    }

    #[test]
    fn every_measured_file_form_is_a_file() {
        for url in [
            "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file",
            "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin",
            "https://www.mediafire.com/file/ipnyzofjcwri357",
            "http://mediafire.com/file/ipnyzofjcwri357",
            "https://app.mediafire.com/file/ipnyzofjcwri357",
            "https://www.mediafire.com/file_premium/ipnyzofjcwri357",
            "https://www.mediafire.com/download/ipnyzofjcwri357",
            "https://www.mediafire.com/download/ipnyzofjcwri357/test-10mb.bin/file",
            "https://www.mediafire.com/download.php?ipnyzofjcwri357",
            "https://www.mediafire.com/view/ipnyzofjcwri357",
            "https://www.mediafire.com/listen/ipnyzofjcwri357/song.mp3/file",
            "https://www.mediafire.com/watch/ipnyzofjcwri357",
            "https://WWW.MEDIAFIRE.COM/file/ipnyzofjcwri357",
        ] {
            assert_eq!(parse(url), file("ipnyzofjcwri357"), "{url}");
        }
    }

    #[test]
    fn a_bare_key_is_undecided_and_a_list_is_a_list() {
        for url in [
            "https://www.mediafire.com/?ipnyzofjcwri357",
            "https://mfi.re/?ipnyzofjcwri357",
            "https://app.mediafire.com/ipnyzofjcwri357",
        ] {
            assert_eq!(
                parse(url),
                Some(Address::Bare {
                    key: "ipnyzofjcwri357".to_owned()
                }),
                "{url}"
            );
        }
        assert_eq!(
            parse("https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx"),
            Some(Address::Keys(vec![
                "ipnyzofjcwri357".to_owned(),
                "8ipst0t9u6sibpx".to_owned()
            ]))
        );
        assert_eq!(
            parse("https://www.mediafire.com/download.php?ipnyzofjcwri357,8ipst0t9u6sibpx"),
            None
        );
    }

    #[test]
    fn folders_are_folders_whatever_follows_the_key() {
        for url in [
            "https://www.mediafire.com/folder/rww7bhhi0yc1l",
            "https://www.mediafire.com/folder/rww7bhhi0yc1l/Droidfeats",
            "https://www.mediafire.com/folder/rww7bhhi0yc1l/shared",
            "https://app.mediafire.com/folder/rww7bhhi0yc1l",
        ] {
            assert_eq!(
                parse(url),
                Some(Address::Folder {
                    key: "rww7bhhi0yc1l".to_owned()
                }),
                "{url}"
            );
        }
    }

    #[test]
    fn what_is_not_an_address_is_refused() {
        for url in [
            "https://www.mediafire.com/",
            "https://www.mediafire.com/file/abc123",
            "https://www.mediafire.com/file/ABCDEFGHIJKLMNO",
            "https://www.mediafire.com/file/ipnyzofjcwri357x",
            "https://www.mediafire.com/upgrade/get_plan.php",
            "https://www.mediafire.com/?not-a-key",
            "https://www.mediafire.com/?ipnyzofjcwri357&foo=bar",
            "https://mediafire.com.evil.test/file/ipnyzofjcwri357",
            "https://x@www.mediafire.com/file/ipnyzofjcwri357",
            "ftp://www.mediafire.com/file/ipnyzofjcwri357",
            "https://download1514.mediafire.com/token/ipnyzofjcwri357/test-10mb.bin",
            "https://example.com/file/ipnyzofjcwri357",
            "not a url",
        ] {
            assert_eq!(parse(url), None, "{url}");
        }
    }

    #[test]
    fn the_delivery_hosts_are_recognised_exactly() {
        assert!(is_download_host("download1514.mediafire.com"));
        assert!(is_download_host("download2269.mediafirecdn.com"));
        assert!(!is_download_host("download.mediafire.com"));
        assert!(!is_download_host("www.mediafire.com"));
        assert!(!is_download_host("download1514.mediafire.com.evil.test"));
        assert!(!is_download_host("xdownload1514.mediafire.com"));
    }

    #[test]
    fn the_error_number_is_read_from_the_redirect_target() {
        assert_eq!(
            error_number("https://www.mediafire.com/error.php?errno=320&origin=download"),
            Some(320)
        );
        assert_eq!(
            error_number("https://www.mediafire.com/file/ipnyzofjcwri357"),
            None
        );
        assert_eq!(error_number("https://www.mediafire.com/error.php"), None);
    }

    #[test]
    fn a_file_link_carries_the_name_when_there_is_one() {
        assert_eq!(
            file_link("ipnyzofjcwri357", Some("test 10mb.bin")),
            "https://www.mediafire.com/file/ipnyzofjcwri357/test%2010mb.bin/file"
        );
        assert_eq!(
            file_link("ipnyzofjcwri357", None),
            "https://www.mediafire.com/file/ipnyzofjcwri357"
        );
        assert_eq!(
            parse(&file_link("ipnyzofjcwri357", Some("a/b.bin"))),
            file("ipnyzofjcwri357")
        );
    }
}
