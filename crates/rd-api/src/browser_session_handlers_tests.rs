//! What the service accepts from the extension (RD-120-45): cookies of the request's scope,
//! in the Netscape rows the account's jar reads, and nothing of any other site.

use url::Url;

use super::cookies_in_scope;

fn scope() -> Url {
    "https://ddownload.com/".parse().expect("scope")
}

/// The refusal's code, read off its debug form; the body is `ApiError`'s own business.
fn code(content: &str) -> String {
    let error = cookies_in_scope(content, &scope()).expect_err("refused");
    format!("{error:?}")
}

#[test]
fn the_scopes_own_cookies_are_accepted_and_counted() {
    let content = ".ddownload.com\tTRUE\t/\tTRUE\t2000000000\txfss\tabc\n\
                   #HttpOnly_ddownload.com\tFALSE\t/\tTRUE\t0\tlogin\tme";
    assert_eq!(cookies_in_scope(content, &scope()).expect("accepted"), 2);
}

#[test]
fn one_cookie_of_another_site_refuses_the_whole_set() {
    for foreign in [
        ".evil.tld\tTRUE\t/\tTRUE\t0\tsession\tabc",
        // Look-alikes the suffix rule must not let through.
        "evil-ddownload.com\tFALSE\t/\tTRUE\t0\tsession\tabc",
        "ddownload.com.evil.tld\tFALSE\t/\tTRUE\t0\tsession\tabc",
        // A subdomain the account's jar would not load for this scope either.
        "files.ddownload.com\tFALSE\t/\tTRUE\t0\tsession\tabc",
    ] {
        let content = format!(".ddownload.com\tTRUE\t/\tTRUE\t0\txfss\tabc\n{foreign}");
        assert!(
            code(&content).contains("browser_session.cookie_outside_scope"),
            "{foreign}"
        );
    }
}

#[test]
fn anything_but_netscape_rows_is_refused() {
    assert!(code("xfss=abc; login=me").contains("browser_session.cookies_invalid"));
    assert!(
        code(".ddownload.com\tTRUE\t/\tTRUE\t0\t\tabc").contains("browser_session.cookies_invalid")
    );
    assert!(code("# only a comment\n\n").contains("browser_session.cookies_empty"));
}

/// A public suffix above the scope would carry the session to every site below it
/// (RD-120-49); it has its own code, so the extension can say which rule refused.
#[test]
fn a_public_suffix_row_refuses_the_whole_set_with_its_own_code() {
    for (scope, suffix) in [
        ("https://ddownload.com/", ".com"),
        ("https://example.co.uk/", ".co.uk"),
    ] {
        let scope: Url = scope.parse().expect("scope");
        let host = scope.host_str().expect("host");
        let content =
            format!(".{host}\tTRUE\t/\tTRUE\t0\txfss\tabc\n{suffix}\tTRUE\t/\tTRUE\t0\tsid\tx");
        let error = cookies_in_scope(&content, &scope).expect_err("refused");
        assert!(
            format!("{error:?}").contains("browser_session.cookie_public_suffix"),
            "{suffix}: {error:?}"
        );
    }
}
