//! Tests of the cookie file: containment, privacy and the stable error codes.

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
        let file =
            written(host, true, &content, &format!("https://{host}/watch")).expect("cookie file");
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
    let wide =
        written("www.example.com", true, parent, "https://www.example.com/v").expect("cookie file");
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
