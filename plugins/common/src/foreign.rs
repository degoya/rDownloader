//! The addresses a plugin's `claim` must refuse, for its unit tests (RD-1280-21).
//!
//! Twelve resolvers and crawlers tested the same four refusals each in their own words: a
//! hoster nobody else's, their own host as the prefix of a stranger's, a user-info part in front
//! of it, and a scheme other than http(s). The list and the disguises are written here once; a
//! plugin's test keeps only the refusals that are its own (its root page, its API host, a folder
//! where a file belongs). The host's conformance check asks the same question of a built
//! component with its own `FOREIGN_LINKS`, which the first two entries here repeat.

use url::Url;

/// Addresses on hosts no cloud or crawler plugin declares.
pub const FOREIGN_LINKS: [&str; 3] = [
    "https://conformance.invalid/some/file.bin",
    "https://cdn.example.org/a/b/c.zip",
    "https://ddownload.com/f/abc",
];

/// Every address a plugin must refuse that is built around `own`, an address it claims: its
/// host as the prefix of a stranger's (`<host>.evil.test`), the same address behind a user-info
/// part and under `ftp://` — followed by the [`FOREIGN_LINKS`].
///
/// # Panics
///
/// When `own` is not an absolute http(s) address with a host; it is a test's own constant.
#[must_use]
pub fn around(own: &str) -> Vec<String> {
    let parsed = Url::parse(own).expect("an absolute address");
    let host = parsed.host_str().expect("an address with a host");
    let mut lookalike = parsed.clone();
    lookalike
        .set_host(Some(&format!("{host}.evil.test")))
        .expect("a host that takes a suffix");
    let mut with_user = parsed.clone();
    with_user.set_username("x").expect("a host-based address");
    let mut other_scheme = parsed;
    other_scheme
        .set_scheme("ftp")
        .expect("an http(s) address that ftp can replace");
    [lookalike, with_user, other_scheme]
        .iter()
        .map(Url::to_string)
        .chain(FOREIGN_LINKS.iter().map(|link| (*link).to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{FOREIGN_LINKS, around};

    #[test]
    fn the_disguises_keep_everything_but_what_they_disguise() {
        let links = around("https://app.box.com/s/abc/file/1?password=x");
        assert_eq!(
            links[0],
            "https://app.box.com.evil.test/s/abc/file/1?password=x"
        );
        assert_eq!(links[1], "https://x@app.box.com/s/abc/file/1?password=x");
        assert_eq!(links[2], "ftp://app.box.com/s/abc/file/1?password=x");
        assert_eq!(links[3..], FOREIGN_LINKS.map(str::to_owned));
    }

    #[test]
    fn a_fragment_address_keeps_its_fragment() {
        let links = around("https://my.pcloud.com/#/filemanager?folder=1");
        assert_eq!(
            links[0],
            "https://my.pcloud.com.evil.test/#/filemanager?folder=1"
        );
    }
}
