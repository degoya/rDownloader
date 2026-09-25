//! The Netscape `cookies.txt` format: parsing, host scoping and serialisation.
//!
//! One implementation, shared by everything that touches stored cookie material. The HTTP
//! engine imports rows into a `reqwest` jar; the media runner writes a filtered subset back
//! out for `yt-dlp --cookies`. Keeping the format in one place is deliberate: a cookie file
//! parser is a security boundary, and two copies of one are two copies that drift.

use serde::{Deserialize, Serialize};

/// Longest cookie blob accepted, matching [`crate::MAX_AUTH_COOKIES`].
pub const MAX_COOKIE_FILE: usize = 4 * 1024 * 1024;

/// Why a cookie blob was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieFileError {
    /// Larger than [`MAX_COOKIE_FILE`].
    TooLarge,
    /// A tab-separated row that does not have seven fields.
    MalformedRow,
    /// A `Cookie:` header pair without `=`, or carrying control characters.
    MalformedHeader,
    /// Parsed successfully but contained no usable cookie.
    Empty,
}

impl core::fmt::Display for CookieFileError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::TooLarge => "cookie file exceeds the size limit",
            Self::MalformedRow => "invalid Netscape cookie row",
            Self::MalformedHeader => "invalid Cookie header value",
            Self::Empty => "cookie import contains no cookies",
        };
        formatter.write_str(text)
    }
}

impl core::error::Error for CookieFileError {}

/// One cookie, as the Netscape format models it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CookieRow {
    /// Lowercased host, always stored without the format's leading dot.
    pub domain: String,
    /// The leading dot in the source: the cookie covers subdomains too.
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    /// Unix expiry; `0` marks a session cookie with no wall-clock lifetime.
    pub expires: i64,
    pub name: String,
    pub value: String,
}

impl CookieRow {
    /// Whether this cookie may be sent to `host`.
    ///
    /// The leading-dot rule is what keeps a cookie for `example.com` away from
    /// `evil-example.com` and `example.com.evil.tld`. A row without the subdomain flag
    /// matches its own host only.
    #[must_use]
    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host == self.domain {
            return true;
        }
        self.include_subdomains && host.ends_with(&format!(".{}", self.domain))
    }

    /// The row in Netscape form, without a trailing newline.
    #[must_use]
    pub fn to_netscape(&self) -> String {
        let domain = if self.include_subdomains {
            format!(".{}", self.domain)
        } else {
            self.domain.clone()
        };
        let flag = |value: bool| if value { "TRUE" } else { "FALSE" };
        format!(
            "{domain}\t{}\t{}\t{}\t{}\t{}\t{}",
            flag(self.include_subdomains),
            self.path,
            flag(self.secure),
            self.expires,
            self.name,
            self.value
        )
    }
}

/// Parses either a Netscape cookie file or a browser `Cookie:` header.
///
/// The format is detected the same way everywhere: a line with seven tab-separated fields
/// means Netscape, anything else is treated as a header. A header carries no domain of its
/// own, so `header_host` supplies one and the cookies are bound to it and its subdomains.
/// That reach is provisional: the one writer, `rd-media`'s yt-dlp cookie file, rewrites every
/// row to the reach of its profile's `rd_http::CookieScope` (RD-120-52), which drops the
/// subdomains unless the profile includes them.
pub fn parse(content: &str, header_host: &str) -> Result<Vec<CookieRow>, CookieFileError> {
    if content.len() > MAX_COOKIE_FILE {
        return Err(CookieFileError::TooLarge);
    }
    let rows = if content.lines().any(|line| line.split('\t').count() >= 7) {
        parse_netscape(content)?
    } else {
        parse_header(content, header_host)?
    };
    if rows.is_empty() {
        return Err(CookieFileError::Empty);
    }
    Ok(rows)
}

fn parse_netscape(content: &str) -> Result<Vec<CookieRow>, CookieFileError> {
    let mut rows = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        // `#HttpOnly_` is a real row wearing a comment prefix; every other `#` is a comment.
        if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
            continue;
        }
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 7 {
            return Err(CookieFileError::MalformedRow);
        }
        let raw_domain = fields[0];
        let include_subdomains =
            raw_domain.starts_with('.') || fields[1].eq_ignore_ascii_case("true");
        let domain = raw_domain.trim_start_matches('.').to_ascii_lowercase();
        if domain.is_empty() {
            return Err(CookieFileError::MalformedRow);
        }
        let path = if fields[2].is_empty() { "/" } else { fields[2] };
        rows.push(CookieRow {
            domain,
            include_subdomains,
            path: path.to_owned(),
            secure: fields[3].eq_ignore_ascii_case("true"),
            // An unparseable expiry becomes a session cookie rather than a rejection:
            // browsers write plenty of odd values there and none of them are a risk.
            expires: fields[4].parse::<i64>().unwrap_or(0),
            name: fields[5].to_owned(),
            value: fields[6].to_owned(),
        });
    }
    Ok(rows)
}

fn parse_header(content: &str, host: &str) -> Result<Vec<CookieRow>, CookieFileError> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Err(CookieFileError::MalformedHeader);
    }
    let content = content.trim();
    let content = content.strip_prefix("Cookie:").unwrap_or(content).trim();
    let mut rows = Vec::new();
    for pair in content.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, value) = pair
            .split_once('=')
            .ok_or(CookieFileError::MalformedHeader)?;
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(CookieFileError::MalformedHeader);
        }
        rows.push(CookieRow {
            domain: host.clone(),
            include_subdomains: true,
            path: "/".to_owned(),
            // A header export says nothing about the Secure flag, and assuming `true` would
            // hide the cookie from an http:// URL the user explicitly asked for.
            secure: false,
            expires: 0,
            name: name.to_owned(),
            value: value.to_owned(),
        });
    }
    Ok(rows)
}

/// Serialises rows as a Netscape cookie file, including the header line curl, yt-dlp and
/// every browser extension look for.
#[must_use]
pub fn to_netscape_file(rows: &[CookieRow]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for row in rows {
        out.push_str(&row.to_netscape());
        out.push('\n');
    }
    out
}

/// The earliest wall-clock expiry across the rows, so an imported browser session inherits
/// the lifetime the browser gave it. Session cookies (`0`) have none and are skipped.
#[must_use]
pub fn earliest_expiry(rows: &[CookieRow]) -> Option<i64> {
    rows.iter()
        .map(|row| row.expires)
        .filter(|expires| *expires > 0)
        .min()
}

#[cfg(test)]
mod tests {
    use super::{CookieFileError, earliest_expiry, parse, to_netscape_file};

    fn netscape(rows: &str) -> String {
        format!("# Netscape HTTP Cookie File\n{rows}")
    }

    #[test]
    fn parses_a_netscape_row_and_strips_the_leading_dot() {
        let rows = parse(
            &netscape(".example.com\tTRUE\t/\tTRUE\t1700000000\tsid\tabc\n"),
            "example.com",
        )
        .expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].domain, "example.com");
        assert!(rows[0].include_subdomains);
        assert!(rows[0].secure);
        assert_eq!(rows[0].value, "abc");
    }

    #[test]
    fn http_only_rows_are_data_not_comments() {
        let rows = parse(
            &netscape("#HttpOnly_example.com\tFALSE\t/\tTRUE\t0\tsid\tabc\n# a real comment\n"),
            "example.com",
        )
        .expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "sid");
    }

    #[test]
    fn host_matching_follows_the_leading_dot_rule() {
        let rows = parse(
            &netscape(".example.com\tTRUE\t/\tTRUE\t0\tsid\tabc\n"),
            "example.com",
        )
        .expect("rows");
        assert!(rows[0].matches_host("example.com"));
        assert!(rows[0].matches_host("cdn.example.com"));
        // The bug this locks in: a bare suffix compare would match all three.
        for host in ["evil-example.com", "example.com.evil.tld", "notexample.com"] {
            assert!(!rows[0].matches_host(host), "{host}");
        }
    }

    #[test]
    fn a_row_without_the_dot_does_not_cover_subdomains() {
        let rows = parse(
            &netscape("example.com\tFALSE\t/\tTRUE\t0\tsid\tabc\n"),
            "example.com",
        )
        .expect("rows");
        assert!(rows[0].matches_host("example.com"));
        assert!(!rows[0].matches_host("cdn.example.com"));
    }

    #[test]
    fn header_cookies_are_bound_to_the_supplied_host() {
        let rows = parse("Cookie: sid=abc; theme=dark", "example.com").expect("rows");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.domain == "example.com"));
        assert!(!rows[0].matches_host("other.tld"));
    }

    #[test]
    fn a_truncated_row_inside_a_netscape_file_is_refused() {
        // One good row makes the file Netscape, so the short row is a malformed row rather
        // than something to skip past: a partially parsed cookie file is not a cookie file.
        assert_eq!(
            parse(
                &netscape(
                    "example.com\tFALSE\t/\tTRUE\t0\tsid\tabc\n\
                     example.com\tTRUE\t/\n"
                ),
                "example.com",
            ),
            Err(CookieFileError::MalformedRow)
        );
    }

    #[test]
    fn malformed_input_is_refused() {
        // Too few fields to look like Netscape at all, so it is judged as a header.
        assert_eq!(
            parse(&netscape("example.com\tTRUE\t/\n"), "example.com"),
            Err(CookieFileError::MalformedHeader)
        );
        assert_eq!(
            parse("not-a-pair", "example.com"),
            Err(CookieFileError::MalformedHeader)
        );
        assert_eq!(parse("", "example.com"), Err(CookieFileError::Empty));
        assert_eq!(
            parse("sid=abc\u{7}bad", "example.com"),
            Err(CookieFileError::MalformedHeader)
        );
    }

    #[test]
    fn rows_round_trip_through_the_file_format() {
        let original = parse(
            &netscape(
                ".example.com\tTRUE\t/media\tTRUE\t1700000000\tsid\tabc\n\
                 other.tld\tFALSE\t/\tFALSE\t0\tt\t1\n",
            ),
            "example.com",
        )
        .expect("rows");
        let reparsed = parse(&to_netscape_file(&original), "example.com").expect("reparsed");
        assert_eq!(original, reparsed);
    }

    #[test]
    fn earliest_expiry_ignores_session_cookies() {
        let rows = parse(
            &netscape(
                "example.com\tFALSE\t/\tTRUE\t0\tsession\ta\n\
                 example.com\tFALSE\t/\tTRUE\t1700000000\tlater\tb\n\
                 example.com\tFALSE\t/\tTRUE\t1600000000\tsooner\tc\n",
            ),
            "example.com",
        )
        .expect("rows");
        assert_eq!(earliest_expiry(&rows), Some(1_600_000_000));
    }
}
