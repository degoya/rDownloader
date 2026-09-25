//! Criterion 1, first half: every accepted link shape and every live domain is recognised,
//! and nothing that is a page rather than a file is.

use super::{HITFILE, TURBOBIT};
use crate::link::{Link, LinkError, parse};
use crate::matches;

const TB_ID: &str = "a1b2c3d4e5f6";

#[test]
fn every_live_turbobit_domain_is_claimed_in_every_file_shape() {
    for host in TURBOBIT.match_hosts {
        for path in [
            format!("/{TB_ID}.html"),
            format!("/{TB_ID}/Sample%20File%201.pdf.html"),
            format!("/download/free/{TB_ID}"),
            format!("/download/started/{TB_ID}"),
            format!("/download/redirect/0123456789abcdef0123456789abcdef/{TB_ID}/Sample.pdf"),
            format!("/download/redirect/0123456789abcdef0123456789abcdef/{TB_ID}"),
            format!("/{TB_ID}.html?short_domain=trbt.cc"),
            format!("/{TB_ID}.html?from_mirror=1&site_version=1#top"),
        ] {
            let url = format!("https://{host}{path}");
            assert_eq!(
                parse(&TURBOBIT, &url),
                Ok(Link::File(TB_ID.to_owned())),
                "{url}"
            );
            assert!(matches(&TURBOBIT, &url), "{url}");
        }
    }
}

#[test]
fn a_turbobit_folder_is_claimed_as_a_folder() {
    assert_eq!(
        parse(&TURBOBIT, "https://turbobit.net/download/folder/12345"),
        Ok(Link::Folder)
    );
    assert!(matches(
        &TURBOBIT,
        "https://turbobit.net/download/folder/12345"
    ));
    assert_eq!(
        parse(&TURBOBIT, "https://turbobit.net/download/folder/abc"),
        Err(LinkError::Unsupported)
    );
}

#[test]
fn turbobit_refuses_what_is_not_a_file_link() {
    for url in [
        // A bare id is not a Turbobit file link; the site's router wants `.html`.
        format!("https://turbobit.net/{TB_ID}"),
        // Wrong case, wrong length.
        "https://turbobit.net/A1B2C3D4E5F6.html".to_owned(),
        "https://turbobit.net/abc123.html".to_owned(),
        "https://turbobit.net/abcdefghijklm.html".to_owned(),
        // Pages that happen to look like ids.
        "https://turbobit.net/linkchecker.html".to_owned(),
        "https://turbobit.net/rules".to_owned(),
        "https://turbobit.net/".to_owned(),
        "https://turbobit.net/download/free/".to_owned(),
        "https://turbobit.net/download/redirect/nothex/a1b2c3d4e5f6".to_owned(),
        // Foreign and dead hosts are not claimed.
        format!("https://turbobit.com/{TB_ID}.html"),
        format!("https://turbobit.to/{TB_ID}.html"),
        format!("https://turbobit.online/{TB_ID}.html"),
        format!("https://example.com/{TB_ID}.html"),
        format!("ftp://turbobit.net/{TB_ID}.html"),
        // Another brand's link is the other plugin's business.
        "https://hitfile.net/Ab1CdEf".to_owned(),
    ] {
        assert_eq!(parse(&TURBOBIT, &url), Err(LinkError::Unsupported), "{url}");
        assert!(!matches(&TURBOBIT, &url), "{url}");
    }
    assert_eq!(parse(&TURBOBIT, "not a url"), Err(LinkError::Invalid));
    assert_eq!(parse(&TURBOBIT, ""), Err(LinkError::Invalid));
}

#[test]
fn every_live_hitfile_domain_is_claimed_in_both_id_shapes() {
    for host in HITFILE.match_hosts {
        for (path, id) in [
            ("/Ab1CdEf", "Ab1CdEf"),
            ("/0ZGT", "0ZGT"),
            ("/Ab1CdEf/premium-only-sample.rar.html", "Ab1CdEf"),
            ("/0ZGT/name.rar.html", "0ZGT"),
            // The `.html` form is not what `links/check` wants, but a person may paste it.
            ("/Ab1CdEf.html", "Ab1CdEf"),
            ("/download/free/Ab1CdEf", "Ab1CdEf"),
            ("/download/started/Gh2IjKl", "Gh2IjKl"),
            (
                "/download/redirect/0123456789abcdef0123456789abcdef/Gh2IjKl/free-sample.zip",
                "Gh2IjKl",
            ),
            ("/Gh2IjKl?short_domain=htfl.net", "Gh2IjKl"),
        ] {
            let url = format!("https://{host}{path}");
            assert_eq!(
                parse(&HITFILE, &url),
                Ok(Link::File(id.to_owned())),
                "{url}"
            );
        }
    }
}

#[test]
fn hitfile_ids_keep_their_case_and_pages_are_not_ids() {
    assert_eq!(
        parse(&HITFILE, "https://hitfile.net/Uw1TVhP"),
        Ok(Link::File("Uw1TVhP".to_owned()))
    );
    assert_ne!(
        parse(&HITFILE, "https://hitfile.net/uw1tvhp"),
        Ok(Link::File("Uw1TVhP".to_owned()))
    );
    for page in [
        "abuse",
        "faq",
        "files",
        "impressum",
        "linkchecker",
        "premium",
        "reseller",
        "rules",
        "favicon",
        "locale",
        "login",
        "reg",
        "upload",
        "api",
        "error",
        "Login",
    ] {
        let url = format!("https://hitfile.net/{page}");
        assert_eq!(parse(&HITFILE, &url), Err(LinkError::Unsupported), "{url}");
    }
    for url in [
        "https://hitfile.net/abc",
        "https://hitfile.net/abcdefgh",
        "https://hitfile.net/Ab1CdEf/name.rar",
        "https://hitfile.to/Ab1CdEf",
        "https://turbobit.net/Ab1CdEf",
    ] {
        assert_eq!(parse(&HITFILE, url), Err(LinkError::Unsupported), "{url}");
    }
    assert_eq!(
        parse(&HITFILE, "https://hitfile.net/download/folder/77"),
        Ok(Link::Folder)
    );
}

#[test]
fn the_canonical_link_differs_between_the_brands() {
    assert_eq!(
        TURBOBIT.canonical_link("a1b2c3d4e5f6"),
        "https://turbobit.net/a1b2c3d4e5f6.html"
    );
    assert_eq!(
        HITFILE.canonical_link("Ab1CdEf"),
        "https://hitfile.net/Ab1CdEf"
    );
}

#[test]
fn a_direct_link_may_only_point_at_the_site_or_its_subdomains() {
    assert!(TURBOBIT.owns_host("turbobit.net"));
    assert!(TURBOBIT.owns_host("s351.turbobit.net"));
    assert!(TURBOBIT.owns_host("S351.TURBOBIT.NET"));
    assert!(!TURBOBIT.owns_host("turbobit.net.evil.test"));
    assert!(!TURBOBIT.owns_host("notturbobit.net"));
    assert!(!TURBOBIT.owns_host("turb.pw"));
    assert!(HITFILE.owns_host("s335.hitfile.net"));
    assert!(!HITFILE.owns_host("hitfile.ru"));
}
