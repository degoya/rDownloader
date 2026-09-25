//! What this plugin claims, and how a share address is taken apart.
//!
//! Deliberately not the `url` crate: a plugin component links nothing outside
//! `rdownloader:plugin`. What is needed here is a host, a share token and — when the share is
//! protected — the password somebody appended to the address; all three are decisions about
//! text, and every one of them is tested.

/// A share address, taken apart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Share {
    /// `http` or `https`, lowercase.
    pub scheme: String,
    /// Host and optional port, lowercase.
    pub authority: String,
    /// The part of the path in front of `/s/<token>`, empty for a Nextcloud at the site root.
    /// A Nextcloud served under `/cloud` keeps it, because every endpoint hangs off it.
    pub base: String,
    /// The share token.
    pub token: String,
    /// The share password, when the address carried one after `#`.
    pub password: Option<String>,
}

impl Share {
    /// The origin plus the installation's base path: what every endpoint hangs off.
    #[must_use]
    pub fn root(&self) -> String {
        format!("{}://{}{}", self.scheme, self.authority, self.base)
    }

    /// The public DAV endpoint Nextcloud 29 and later serve, without a trailing slash.
    #[must_use]
    pub fn modern_endpoint(&self) -> String {
        format!("{}/public.php/dav/files/{}", self.root(), self.token)
    }

    /// The endpoint older Nextcloud and every ownCloud serve, without a trailing slash.
    #[must_use]
    pub fn legacy_endpoint(&self) -> String {
        format!("{}/public.php/webdav", self.root())
    }
}

/// Whether this plugin claims `url`, answered from the address alone and reaching nothing.
///
/// `/s/<token>` is not a Nextcloud-specific shape — plenty of sites use a two-letter path
/// segment — so this claim is a guess, and being wrong is expected. That is survivable only
/// because `crawl` can answer `unsupported` and have the address carried on to the next
/// crawler (RD-107-05, host gap 3); before that, one wrong guess ended the link.
#[must_use]
pub fn claim(url: &str) -> Option<Share> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    // The password rides after `#`, which never reaches a server: a fragment is not sent.
    let (before_fragment, fragment) = match tail.split_once('#') {
        Some((before, after)) => (before, Some(after)),
        None => (tail, None),
    };
    let path = before_fragment.split('?').next().unwrap_or("");
    // `/index.php/s/<token>` is the same share as `/s/<token>`; Nextcloud serves both and
    // which one a person copied depends on the instance's URL rewriting.
    let path = path.replace("/index.php/s/", "/s/");
    let (base, after) = path.rsplit_once("/s/")?;
    let token: String = after
        .split('/')
        .next()
        .unwrap_or_default()
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect();
    // Nextcloud tokens are fifteen characters; anything very short is some other site's `/s/`.
    if token.len() < 8
        || after
            .trim_start_matches(&token)
            .trim_matches('/')
            .contains('/')
    {
        return None;
    }
    let password = fragment
        .map(decode)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Some(Share {
        scheme,
        authority: authority.to_ascii_lowercase(),
        base: base.trim_end_matches('/').to_owned(),
        token,
        password,
    })
}

/// Percent-encodes one path segment for a DAV address.
///
/// An allowlist rather than a denylist: a file name comes from somebody else's server, and
/// guessing which characters a given server tolerates is how a request ends up somewhere
/// nobody meant.
#[must_use]
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Percent-decodes text, leaving anything malformed as it was.
#[must_use]
pub fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Puts the login the files of a protected share need in front of an address (RD-108-07).
///
/// The *user name* only, and never the password. A crawler has no way to hand a credential
/// to the host — `crawled-link` carries an address, a name, a size and a folder, and nothing
/// else — but the user name a public endpoint wants is not a secret and the address is the
/// one channel there is. The host lifts it back out, refuses an address that carries a
/// password beside it, and stores the address without either; the password it already has,
/// from the fragment of the address the person pasted.
///
/// A login that is not plain is refused rather than encoded: the two this plugin ever sends
/// are `anonymous` and a share token, both of which are already nothing but letters, digits,
/// `-` and `_`.
#[must_use]
pub fn with_login(url: &str, login: &str) -> String {
    if login.is_empty()
        || !login
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return url.to_owned();
    }
    match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{login}@{rest}"),
        None => url.to_owned(),
    }
}

/// Base64, for the one `Authorization: Basic` header a protected share needs.
///
/// Sixteen lines rather than a dependency: a plugin component may import nothing outside
/// `rdownloader:plugin`, and pulling a crate in for this would be a whole dependency to keep
/// signed and audited for four bytes of arithmetic.
#[must_use]
pub fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let packed = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let position = (packed >> (18 - index * 6)) & 0x3f;
                out.push(ALPHABET[position as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base64, claim, encode_segment, with_login};

    #[test]
    fn both_share_spellings_are_claimed_and_name_the_same_endpoints() {
        let plain = claim("https://cloud.example.org/s/abcdefghijklmno").expect("a share");
        let with_index =
            claim("https://cloud.example.org/index.php/s/abcdefghijklmno").expect("a share");
        assert_eq!(plain.token, "abcdefghijklmno");
        assert_eq!(plain.token, with_index.token);
        assert_eq!(
            plain.modern_endpoint(),
            "https://cloud.example.org/public.php/dav/files/abcdefghijklmno"
        );
        assert_eq!(
            with_index.legacy_endpoint(),
            "https://cloud.example.org/public.php/webdav"
        );
    }

    /// A Nextcloud served under a path keeps it: every endpoint hangs off the installation
    /// root, not off the host.
    #[test]
    fn an_installation_under_a_path_keeps_it() {
        let share = claim("https://example.org/cloud/s/abcdefghijklmno/").expect("a share");
        assert_eq!(share.base, "/cloud");
        assert_eq!(
            share.modern_endpoint(),
            "https://example.org/cloud/public.php/dav/files/abcdefghijklmno"
        );
    }

    /// The password rides in the fragment, which is never sent to a server.
    #[test]
    fn a_password_after_the_hash_is_read_and_decoded() {
        let share =
            claim("https://cloud.example.org/s/abcdefghijklmno#let%20me%20in").expect("a share");
        assert_eq!(share.password.as_deref(), Some("let me in"));
        // Without one there is simply none, which is what makes the refusal specific.
        let bare = claim("https://cloud.example.org/s/abcdefghijklmno").expect("a share");
        assert_eq!(bare.password, None);
    }

    #[test]
    fn an_address_that_is_not_a_share_is_not_claimed() {
        // Too short to be a Nextcloud token.
        assert!(claim("https://example.org/s/abc").is_none());
        // A path below the share: that is a file somebody else resolves.
        assert!(claim("https://example.org/s/abcdefghijklmno/download/x").is_none());
        assert!(claim("ftp://example.org/s/abcdefghijklmno").is_none());
        assert!(claim("https://example.org/share/abcdefghijklmno").is_none());
        assert!(claim("https://me:pw@example.org/s/abcdefghijklmno").is_none());
    }

    /// The address a protected share's files are handed back at carries the user name the
    /// public endpoint wants — and nothing else. A password in there would be a credential
    /// written into a database column, a REST answer and every log line that ever prints a
    /// link, which is exactly what RD-108-07 is about.
    #[test]
    fn a_protected_share_hands_back_its_login_and_never_its_password() {
        assert_eq!(
            with_login(
                "https://cloud.example.org/public.php/dav/files/abc/x.bin",
                "anonymous"
            ),
            "https://anonymous@cloud.example.org/public.php/dav/files/abc/x.bin"
        );
        // The legacy endpoint wants the share token as the user name.
        assert_eq!(
            with_login(
                "https://cloud.example.org/public.php/webdav/x.bin",
                "abcdefghijklmno"
            ),
            "https://abcdefghijklmno@cloud.example.org/public.php/webdav/x.bin"
        );
        // Anything that is not a plain login is refused rather than encoded: a `:` there
        // would read as a password to everything downstream.
        assert_eq!(
            with_login("https://cloud.example.org/x.bin", "anonymous:s3cret"),
            "https://cloud.example.org/x.bin"
        );
        assert_eq!(
            with_login("https://cloud.example.org/x.bin", ""),
            "https://cloud.example.org/x.bin"
        );
        assert_eq!(with_login("not a url", "anonymous"), "not a url");
    }

    #[test]
    fn a_name_is_encoded_for_the_address_and_nothing_is_guessed_at() {
        assert_eq!(encode_segment("Season 1"), "Season%201");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
        assert_eq!(encode_segment("plain-name_1.bin"), "plain-name_1.bin");
    }

    /// The four bytes of arithmetic the `Authorization` header rests on, against the
    /// canonical vectors from RFC 4648.
    #[test]
    fn base64_matches_the_specification() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"anonymous:secret"), "YW5vbnltb3VzOnNlY3JldA==");
    }
}
