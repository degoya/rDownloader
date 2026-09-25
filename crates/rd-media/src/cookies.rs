//! Materialises an authentication profile's cookies for `yt-dlp --cookies` (RD-080-04).
//!
//! yt-dlp cannot be handed a cookie jar; it wants a file. That file is the credential, so
//! three properties are non-negotiable and each is enforced here rather than by the caller:
//!
//! * **Only cookies for the target site are written.** The stored profile is scoped to a
//!   host, but a browser export routinely carries rows for analytics and CDN domains that
//!   happened to be in the same jar. Handing those to an extractor would send one site's
//!   session to another. Rows are filtered, not merely trusted, by the rule every cookie
//!   import follows, `rd_http::CookieScope` (RD-120-52): no public suffix, no widening past
//!   the profile's scope, subdomains only when the profile includes them.
//! * **The file is private and short-lived.** It is created with mode `0o600` on Unix,
//!   inside the process's temp directory, and it is deleted when the guard drops — which
//!   covers success, failure and cancellation alike, because the guard lives on the stack
//!   of the whole run.
//! * **The path, not the contents, is what reaches the process table.** Cookie values never
//!   appear in an argument, and the file name carries no site or profile identity.

use std::path::Path;

use rd_core::{AuthMethod, AuthProfile, CookieRow};
use rd_http::CookieScope;
use secrecy::ExposeSecret;
use tempfile::{Builder, NamedTempFile};
use url::Url;

/// A cookie file that deletes itself when dropped.
///
/// Held by value for the duration of a download; every exit path from the runner drops it,
/// including the early `return` the cancellation branch takes.
///
/// `Debug` prints the path only — the contents are the credential.
#[derive(Debug)]
pub struct CookieFile {
    handle: NamedTempFile,
}

impl CookieFile {
    /// Path to hand to `--cookies`.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.handle.path()
    }
}

/// Why cookies could not be materialised.
///
/// Every variant is reported with a stable code and *without* the cookie material; the
/// runner turns these into a `Failure` the UI translates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieError {
    /// The pinned profile is missing, disabled or past its expiry.
    ProfileUnusable,
    /// The profile does not carry cookies (Basic and Bearer cannot be given to yt-dlp).
    NotCookieBased,
    /// The stored blob could not be read from the vault.
    SecretUnavailable,
    /// The blob is not a cookie file or header.
    Malformed,
    /// Parsed fine, but nothing in it applies to this page's host.
    NoCookiesForHost,
    /// The file could not be created.
    WriteFailed,
}

impl CookieError {
    /// Stable error code, following the `<domain>.<subject>_<condition>` convention.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ProfileUnusable => "media.cookie_profile_unusable",
            Self::NotCookieBased => "media.cookie_profile_not_cookies",
            Self::SecretUnavailable => "media.cookie_secret_unavailable",
            Self::Malformed => "media.cookie_profile_malformed",
            Self::NoCookiesForHost => "media.cookie_scope_empty",
            Self::WriteFailed => "media.cookie_write_failed",
        }
    }

    /// English fallback message; the UI translates by `code`.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::ProfileUnusable => "The selected cookie profile is missing, disabled or expired",
            Self::NotCookieBased => "The selected profile does not store browser cookies",
            Self::SecretUnavailable => "The stored cookies could not be read",
            Self::Malformed => "The stored cookies are not a valid cookie file",
            Self::NoCookiesForHost => "The profile has no cookies for this site",
            Self::WriteFailed => "The temporary cookie file could not be written",
        }
    }
}

/// Keeps only the rows that may be sent to `page_url`'s host, each in the form `scope` stores
/// it.
///
/// This is the containment step. It is separated from the I/O so it can be tested directly,
/// because "a foreign domain's cookie was written into the file" is not a failure that shows
/// up in an integration test — it shows up in somebody else's access log.
///
/// The rule is the HTTP engine's, not a copy of it (RD-120-52): a row must pass
/// [`CookieScope::admit`], and one that does is written for the scope's host — never for the
/// parent domain it came with — reaching subdomains only when
/// [`CookieScope::reaches_subdomains`] says so. Header cookies, which `rd_core` binds to the
/// scope host with subdomains, get that same reach. A refused row is dropped rather than
/// refusing the file, because stripping a browser export's foreign rows is this step's job.
#[must_use]
pub fn rows_for_url(rows: Vec<CookieRow>, scope: &CookieScope, page_url: &Url) -> Vec<CookieRow> {
    let Some(host) = page_url.host_str() else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|row| scope.admit(&row.domain).is_ok())
        .map(|row| CookieRow {
            domain: scope.host().to_owned(),
            include_subdomains: scope.reaches_subdomains(),
            ..row
        })
        .filter(|row| row.matches_host(host))
        .collect()
}

/// Builds a private, self-deleting cookie file for `page_url` from `profile`.
///
/// `secret` is the decrypted blob the vault returned for the profile.
pub fn materialize(
    profile: &AuthProfile,
    secret: &secrecy::SecretString,
    page_url: &Url,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<CookieFile, CookieError> {
    if !profile.is_usable(now) {
        return Err(CookieError::ProfileUnusable);
    }
    if profile.method != AuthMethod::Cookies {
        return Err(CookieError::NotCookieBased);
    }
    // A header-format blob has no domain of its own; the profile's scope host supplies it,
    // which is the same host the profile was approved for.
    let rows = rd_core::parse_cookie_file(secret.expose_secret(), &profile.scope.host)
        .map_err(|_| CookieError::Malformed)?;
    // The same scope the HTTP engine builds for this profile (`rd-scheduler`'s
    // `assemble_client`).
    let scope = profile
        .scope
        .probe_url()
        .and_then(|url| CookieScope::new(&url, profile.scope.include_subdomains).ok())
        .ok_or(CookieError::ProfileUnusable)?;
    let scoped = rows_for_url(rows, &scope, page_url);
    if scoped.is_empty() {
        return Err(CookieError::NoCookiesForHost);
    }
    write_file(&rd_core::to_netscape_file(&scoped))
}

fn write_file(contents: &str) -> Result<CookieFile, CookieError> {
    use std::io::Write;

    // No site or profile name in the prefix: a temp directory is world-readable, and the
    // file name would otherwise say which site the user has a session for.
    let mut builder = Builder::new();
    builder.prefix("rd-cookies-").suffix(".txt");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o600));
    }
    let mut handle = builder.tempfile().map_err(|_| CookieError::WriteFailed)?;
    handle
        .write_all(contents.as_bytes())
        .map_err(|_| CookieError::WriteFailed)?;
    handle.flush().map_err(|_| CookieError::WriteFailed)?;
    Ok(CookieFile { handle })
}

#[cfg(test)]
mod tests {
    use super::{CookieError, CookieScope, materialize, rows_for_url};
    use chrono::{TimeZone, Utc};
    use rd_core::{AuthMethod, AuthOrigin, AuthProfile, AuthProfileId, AuthScope};
    use secrecy::SecretString;
    use url::Url;

    fn url(input: &str) -> Url {
        input.parse().expect("url")
    }

    fn profile(host: &str) -> AuthProfile {
        scoped_profile(host, true)
    }

    fn scoped_profile(host: &str, include_subdomains: bool) -> AuthProfile {
        AuthProfile {
            id: AuthProfileId::new(),
            name: "test".to_owned(),
            scope: AuthScope::parse(host, include_subdomains).expect("scope"),
            method: AuthMethod::Cookies,
            origin: AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: None,
            certificate_ref: None,
            has_secret: true,
            has_client_certificate: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn example_scope() -> CookieScope {
        CookieScope::new(&url("https://example.com/"), true).expect("scope")
    }

    /// The cookie file yt-dlp would be handed, as written to disk.
    fn written(
        host: &str,
        include_subdomains: bool,
        content: &str,
        page: &str,
    ) -> Result<String, CookieError> {
        let file = materialize(
            &scoped_profile(host, include_subdomains),
            &SecretString::from(content.to_owned()),
            &url(page),
            Utc::now(),
        )?;
        Ok(std::fs::read_to_string(file.path()).expect("read"))
    }

    /// The cookie rows of a written file, without the header comment.
    fn lines(file: &str) -> Vec<&str> {
        file.lines().filter(|line| !line.starts_with('#')).collect()
    }

    // RD-120-52: the three findings of RD-120-49, each against the written file.

    #[test]
    fn a_public_suffix_row_is_not_written() {
        for (host, suffix) in [("example.com", ".com"), ("example.co.uk", ".co.uk")] {
            let content = format!(
                "{suffix}\tTRUE\t/\tTRUE\t0\tleak\tto-every-site\n\
                 .{host}\tTRUE\t/\tTRUE\t0\tsid\tmine\n"
            );
            let file = written(host, true, &content, &format!("https://{host}/watch"))
                .expect("cookie file");
            assert!(!file.contains("to-every-site"), "{suffix}:\n{file}");
            assert_eq!(
                lines(&file),
                [format!(".{host}\tTRUE\t/\tTRUE\t0\tsid\tmine")],
                "{suffix}"
            );
            // Nothing else in the set: the file is refused, not written empty.
            let only_suffix = format!("{suffix}\tTRUE\t/\tTRUE\t0\tleak\tto-every-site\n");
            assert_eq!(
                written(host, true, &only_suffix, &format!("https://{host}/watch")),
                Err(CookieError::NoCookiesForHost),
                "{suffix}"
            );
        }
    }

    #[test]
    fn a_parent_domain_row_is_written_for_the_scope_host() {
        let parent = ".example.com\tTRUE\t/\tTRUE\t0\tsid\tsession\n";
        let narrow = written(
            "www.example.com",
            false,
            parent,
            "https://www.example.com/v",
        )
        .expect("cookie file");
        assert_eq!(
            lines(&narrow),
            ["www.example.com\tFALSE\t/\tTRUE\t0\tsid\tsession"]
        );
        let wide = written("www.example.com", true, parent, "https://www.example.com/v")
            .expect("cookie file");
        assert_eq!(
            lines(&wide),
            [".www.example.com\tTRUE\t/\tTRUE\t0\tsid\tsession"]
        );
        // A sibling of the scope host is not reached through the parent domain.
        assert_eq!(
            written("www.example.com", true, parent, "https://dl.example.com/v"),
            Err(CookieError::NoCookiesForHost)
        );
    }

    #[test]
    fn header_cookies_follow_include_subdomains() {
        let header = "Cookie: sid=abc; theme=dark";
        let host_only =
            written("example.com", false, header, "https://example.com/v").expect("cookie file");
        assert_eq!(
            lines(&host_only),
            [
                "example.com\tFALSE\t/\tFALSE\t0\tsid\tabc",
                "example.com\tFALSE\t/\tFALSE\t0\ttheme\tdark",
            ]
        );
        assert_eq!(
            written("example.com", false, header, "https://www.example.com/v"),
            Err(CookieError::NoCookiesForHost)
        );
        let wide =
            written("example.com", true, header, "https://www.example.com/v").expect("cookie file");
        assert_eq!(
            lines(&wide),
            [
                ".example.com\tTRUE\t/\tFALSE\t0\tsid\tabc",
                ".example.com\tTRUE\t/\tFALSE\t0\ttheme\tdark",
            ]
        );
        // A scope host that is itself a public suffix (an intranet `nas`) stays host-only.
        let intranet = written("nas", true, header, "https://nas/v").expect("cookie file");
        assert!(
            lines(&intranet)
                .iter()
                .all(|line| line.starts_with("nas\tFALSE\t")),
            "{intranet}"
        );
    }

    fn rows(content: &str) -> Vec<rd_core::CookieRow> {
        rd_core::parse_cookie_file(content, "example.com").expect("rows")
    }

    const MIXED_JAR: &str = "# Netscape HTTP Cookie File\n\
        .example.com\tTRUE\t/\tTRUE\t0\tsid\tsecret-value\n\
        .tracker.invalid\tTRUE\t/\tTRUE\t0\ttrack\tfollow-me\n\
        cdn.other.tld\tFALSE\t/\tTRUE\t0\tedge\tnope\n";

    #[test]
    fn foreign_domains_are_dropped_from_the_jar() {
        let scoped = rows_for_url(
            rows(MIXED_JAR),
            &example_scope(),
            &url("https://example.com/watch?v=1"),
        );
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].name, "sid");
    }

    #[test]
    fn a_subdomain_page_still_gets_the_wildcard_cookie() {
        let scoped = rows_for_url(
            rows(MIXED_JAR),
            &example_scope(),
            &url("https://www.example.com/watch"),
        );
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].name, "sid");
    }

    #[test]
    fn a_lookalike_host_gets_nothing() {
        for page in [
            "https://evil-example.com/watch",
            "https://example.com.evil.tld/watch",
        ] {
            assert!(
                rows_for_url(rows(MIXED_JAR), &example_scope(), &url(page)).is_empty(),
                "{page}"
            );
        }
    }

    #[test]
    fn the_written_file_holds_only_the_matching_rows() {
        let file = materialize(
            &profile("example.com"),
            &SecretString::from(MIXED_JAR.to_owned()),
            &url("https://example.com/watch"),
            Utc::now(),
        )
        .expect("cookie file");
        let written = std::fs::read_to_string(file.path()).expect("read");
        assert!(written.contains("secret-value"));
        assert!(!written.contains("follow-me"));
        assert!(!written.contains("nope"));
    }

    #[test]
    fn the_file_is_removed_when_the_guard_drops() {
        let path = {
            let file = materialize(
                &profile("example.com"),
                &SecretString::from(MIXED_JAR.to_owned()),
                &url("https://example.com/watch"),
                Utc::now(),
            )
            .expect("cookie file");
            file.path().to_path_buf()
        };
        assert!(!path.exists(), "cookie file outlived its guard");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_not_readable_by_anyone_else() {
        use std::os::unix::fs::PermissionsExt;
        let file = materialize(
            &profile("example.com"),
            &SecretString::from(MIXED_JAR.to_owned()),
            &url("https://example.com/watch"),
            Utc::now(),
        )
        .expect("cookie file");
        let mode = std::fs::metadata(file.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "cookie file is group- or world-accessible");
    }

    #[test]
    fn a_page_with_no_matching_cookie_is_an_error_not_an_empty_file() {
        let error = materialize(
            &profile("example.com"),
            &SecretString::from(MIXED_JAR.to_owned()),
            &url("https://unrelated.tld/watch"),
            Utc::now(),
        )
        .expect_err("should refuse");
        assert_eq!(error, CookieError::NoCookiesForHost);
    }

    #[test]
    fn a_disabled_or_expired_profile_is_refused() {
        let mut disabled = profile("example.com");
        disabled.enabled = false;
        assert_eq!(
            materialize(
                &disabled,
                &SecretString::from(MIXED_JAR.to_owned()),
                &url("https://example.com/watch"),
                Utc::now(),
            )
            .expect_err("disabled profile"),
            CookieError::ProfileUnusable
        );

        let mut expired = profile("example.com");
        expired.expires_at = Some(Utc.timestamp_opt(1, 0).single().expect("timestamp"));
        assert_eq!(
            materialize(
                &expired,
                &SecretString::from(MIXED_JAR.to_owned()),
                &url("https://example.com/watch"),
                Utc::now(),
            )
            .expect_err("expired profile"),
            CookieError::ProfileUnusable
        );
    }

    #[test]
    fn a_non_cookie_profile_is_refused() {
        let mut bearer = profile("example.com");
        bearer.method = AuthMethod::Bearer;
        assert_eq!(
            materialize(
                &bearer,
                &SecretString::from("token".to_owned()),
                &url("https://example.com/watch"),
                Utc::now(),
            )
            .expect_err("bearer profile"),
            CookieError::NotCookieBased
        );
    }
}
