//! The documented link check, against the measured answers.

use plugin_common::{CheckInput, LinkStatus};

use super::{HITFILE, MockHost, TURBOBIT, body_of, hf, json, run, tb};
use crate::check;

fn input(urls: &[&str]) -> CheckInput {
    CheckInput {
        urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        account_id: None,
    }
}

#[test]
fn turbobit_links_are_checked_in_their_canonical_html_form_and_mapped_in_order() {
    let host = MockHost::new(vec![json(200, tb::LINKS_CHECK)]);
    let results = run(check(
        &TURBOBIT,
        &host,
        &input(&[
            "https://turbobit.net/a1b2c3d4e5f6.html",
            "https://turbobit.net/abcdefghijkl.html",
            "https://turbobit.net/download/free/a1b2c3d4e5f6",
            "https://turb.pw/a1b2c3d4e5f6.html",
        ]),
    ))
    .expect("checked");
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("Sample File 1.pdf"));
    assert_eq!(results[1].status, LinkStatus::Offline);
    assert_eq!(results[1].file_name, None);
    // The measurement sent the SPA's intermediate path and got `invalid`; mapped as Unknown.
    assert_eq!(results[2].status, LinkStatus::Unknown);
    assert_eq!(results[3].status, LinkStatus::Online);
    assert_eq!(results[3].url, "https://turb.pw/a1b2c3d4e5f6.html");

    assert_eq!(host.request_count(), 1);
    let body = body_of(&host.request(0));
    // Every link went out as the main site's `.html` form; the short domain was never sent.
    assert!(
        body.starts_with("links=https%3A%2F%2Fturbobit.net%2Fa1b2c3d4e5f6.html%0A"),
        "{body}"
    );
    assert!(!body.contains("turb.pw"), "{body}");
    assert_eq!(body.matches("%0A").count(), 3, "{body}");
}

#[test]
fn hitfile_links_are_checked_without_html_and_the_short_domain_is_rewritten() {
    let host = MockHost::new(vec![json(200, hf::LINKS_CHECK)]);
    let results = run(check(
        &HITFILE,
        &host,
        &input(&[
            "https://hitfile.net/Ab1CdEf",
            "https://hitfile.net/Gh2IjKl",
            "https://hitfile.net/Mn3OpQr",
            "https://hitfile.net/Gh2IjKl.html",
            "https://hil.to/Gh2IjKl",
        ]),
    ))
    .expect("checked");
    assert_eq!(
        results.iter().map(|r| r.status).collect::<Vec<_>>(),
        vec![
            LinkStatus::Online,
            LinkStatus::Online,
            LinkStatus::Offline,
            LinkStatus::Unknown,
            LinkStatus::Online
        ]
    );
    assert_eq!(
        results[0].file_name.as_deref(),
        Some("premium-only-sample.rar")
    );
    let body = body_of(&host.request(0));
    assert!(
        !body.contains(".html"),
        "HitFile's `.html` form is `invalid`: {body}"
    );
    assert!(!body.contains("hil.to"), "{body}");
    assert!(body.contains("hitfile.net%2FGh2IjKl%0A"), "{body}");
}

#[test]
fn a_link_that_is_not_a_file_is_unknown_without_being_sent() {
    let host = MockHost::new(Vec::new());
    let results = run(check(
        &TURBOBIT,
        &host,
        &input(&[
            "https://turbobit.net/rules",
            "not a url",
            "https://example.com/x",
        ]),
    ))
    .expect("checked");
    assert!(results.iter().all(|r| r.status == LinkStatus::Unknown));
    assert_eq!(host.request_count(), 0);
}

#[test]
fn an_answer_naming_another_id_is_not_taken_for_this_link() {
    let host = MockHost::new(vec![json(
        200,
        r#"[{"id":"zzzzzzzzzzzz","url":"x","name":"other","status":"active"}]"#,
    )]);
    let results = run(check(
        &TURBOBIT,
        &host,
        &input(&["https://turbobit.net/a1b2c3d4e5f6.html"]),
    ))
    .expect("checked");
    assert_eq!(results[0].status, LinkStatus::Unknown);
    assert_eq!(results[0].file_name, None);
}

#[test]
fn fifty_links_go_in_one_call_and_the_fifty_first_in_a_second() {
    let entries: Vec<String> = (0..50)
        .map(|n| format!(r#"{{"id":"link{n:08}","url":"x","name":"f","status":"active"}}"#))
        .collect();
    let first = format!("[{}]", entries.join(","));
    let host = MockHost::new(vec![
        json(200, &first),
        json(
            200,
            r#"[{"id":"link00000050","url":"x","name":"f","status":"inactive"}]"#,
        ),
    ]);
    let urls: Vec<String> = (0..51)
        .map(|n| format!("https://turbobit.net/link{n:08}.html"))
        .collect();
    let input = CheckInput {
        urls,
        account_id: None,
    };
    let results = run(check(&TURBOBIT, &host, &input)).expect("checked");
    assert_eq!(results.len(), 51);
    assert_eq!(results[50].status, LinkStatus::Offline);
    assert_eq!(host.request_count(), 2);
}

#[test]
fn a_refused_batch_fails_the_call_with_the_apis_code() {
    let host = MockHost::new(vec![json(429, "")]);
    let failure = run(check(
        &TURBOBIT,
        &host,
        &input(&["https://turbobit.net/a1b2c3d4e5f6.html"]),
    ))
    .expect_err("rate limited");
    assert_eq!(super::code(&failure), "turbobit.rate_limited");
}
