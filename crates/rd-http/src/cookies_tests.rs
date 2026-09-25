//! Which hosts an imported cookie reaches, asked of a real `reqwest` jar (RD-120-49).

use reqwest::cookie::{CookieStore, Jar};
use url::Url;

use super::{CookieDomainRefused, CookieScope, import_cookie_jar};

fn url(text: &str) -> Url {
    text.parse().expect("url")
}

fn provider(text: &str) -> CookieScope {
    CookieScope::provider(&url(text)).expect("scope")
}

/// Whether `jar` sends a cookie called `name` to `target`.
fn sends(jar: &Jar, target: &str, name: &str) -> bool {
    jar.cookies(&url(target)).is_some_and(|header| {
        header
            .to_str()
            .expect("header")
            .split("; ")
            .any(|pair| pair.starts_with(&format!("{name}=")))
    })
}

fn refusal(content: &str, scope: &CookieScope) -> CookieDomainRefused {
    let error = import_cookie_jar(content, scope).expect_err("refused");
    *error
        .downcast_ref::<CookieDomainRefused>()
        .unwrap_or_else(|| panic!("not a domain refusal: {error}"))
}

#[test]
fn imports_header_without_leaking_to_other_domains() {
    let scope = provider("https://ddownload.com/file/abc");
    let jar = import_cookie_jar("Cookie: session=abc; preference=dark", &scope).expect("jar");
    assert!(sends(&jar, "https://ddownload.com/file/abc", "session"));
    assert!(jar.cookies(&url("https://example.com/")).is_none());
}

#[test]
fn header_cookies_cover_every_subdomain_of_the_scope() {
    let scope = provider("https://ddownload.com/");
    let jar = import_cookie_jar("login=me; xfss=session", &scope).expect("jar");
    for target in [
        "https://ddownload.com/abc123xyz",
        "https://api-v2.ddownload.com/api/file/info",
        "https://www.ddownload.com/",
    ] {
        assert!(sends(&jar, target, "xfss"), "{target}");
    }
    let cdn = url("https://eu-hydra5.zeuscdn.org:183/d/x/file.rar");
    assert!(jar.cookies(&cdn).is_none());
}

#[test]
fn rejects_netscape_cookie_outside_scope() {
    let scope = provider("https://ddownload.com/");
    let content = ".example.com\tTRUE\t/\tTRUE\t0\tsession\tabc";
    assert_eq!(refusal(content, &scope), CookieDomainRefused::OutsideScope);
}

/// Finding 1: a row for a public suffix above the scope would reach every site below it.
#[test]
fn a_public_suffix_above_the_scope_is_refused() {
    for (scope, domain) in [
        ("https://ddownload.com/", ".com"),
        ("https://ddownload.com/", "com"),
        ("https://example.co.uk/", ".co.uk"),
        ("https://example.co.uk/", ".uk"),
        // A private-section entry of the list, and a single label the list does not know.
        ("https://someone.github.io/", ".github.io"),
        ("https://nas.lan/", ".lan"),
    ] {
        let good = format!(".{}\tTRUE\t/\tTRUE\t0\tkeep\t1", provider(scope).host());
        let content = format!("{good}\n{domain}\tTRUE\t/\tTRUE\t0\tsession\tabc");
        assert_eq!(
            refusal(&content, &provider(scope)),
            CookieDomainRefused::PublicSuffix,
            "{domain} for {scope}"
        );
    }
}

/// Finding 2: a header cookie follows the profile's `include_subdomains`.
#[test]
fn header_cookies_without_subdomains_reach_the_host_only() {
    let scope = CookieScope::new(&url("https://example.com/"), false).expect("scope");
    let jar = import_cookie_jar("Cookie: session=abc", &scope).expect("jar");
    assert!(sends(&jar, "https://example.com/media/", "session"));
    assert!(!sends(&jar, "https://www.example.com/", "session"));
    assert!(!sends(&jar, "https://dl.example.com/file", "session"));
}

/// Finding 3: a row for a domain above the scope is stored for the scope, not above it.
#[test]
fn a_parent_domain_row_does_not_widen_the_scope() {
    let content = ".example.com\tTRUE\t/\tTRUE\t0\tsession\tabc\n\
                   www.example.com\tFALSE\t/\tTRUE\t0\tlogin\tme";
    let narrow = CookieScope::new(&url("https://www.example.com/"), false).expect("scope");
    let jar = import_cookie_jar(content, &narrow).expect("jar");
    for name in ["session", "login"] {
        assert!(sends(&jar, "https://www.example.com/", name), "{name}");
        for elsewhere in [
            "https://example.com/",
            "https://dl.example.com/",
            "https://a.www.example.com/",
        ] {
            assert!(!sends(&jar, elsewhere, name), "{name} at {elsewhere}");
        }
    }

    let wide = CookieScope::new(&url("https://www.example.com/"), true).expect("scope");
    let jar = import_cookie_jar(content, &wide).expect("jar");
    assert!(sends(&jar, "https://a.www.example.com/", "session"));
    assert!(!sends(&jar, "https://dl.example.com/", "session"));
    assert!(!sends(&jar, "https://example.com/", "session"));
}

/// A scope whose host is itself a public suffix, as an intranet `nas` is, keeps its own
/// cookies — host-only, since a `Domain=nas` would cover every name below it.
#[test]
fn a_public_suffix_host_keeps_its_own_cookies_host_only() {
    let scope = provider("https://nas/");
    let jar = import_cookie_jar("nas\tFALSE\t/\tTRUE\t0\tsession\tabc", &scope).expect("jar");
    assert!(sends(&jar, "https://nas/", "session"));
    assert!(!sends(&jar, "https://other.nas/", "session"));
}
