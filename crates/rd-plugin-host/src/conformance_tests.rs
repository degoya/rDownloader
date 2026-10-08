//! PL-10: the two conformance checks that are logic of their own rather than a call into the
//! loader -- the completeness of the required translations and the over-claim probe.

use super::{FOREIGN_LINKS, claims_no_foreign_link, required_locales_complete};

fn locale(language: &str, codes: &[&str]) -> (String, Vec<u8>) {
    let codes: serde_json::Map<String, serde_json::Value> = codes
        .iter()
        .map(|code| ((*code).to_owned(), "text".into()))
        .collect();
    let json = serde_json::json!({ "codes": codes });
    (language.to_owned(), json.to_string().into_bytes())
}

#[test]
fn a_required_language_must_carry_every_english_code() {
    let english = locale("en", &["demo.one", "demo.two"]);
    let whole = [
        locale("de", &["demo.one", "demo.two"]),
        english.clone(),
        locale("fr", &["demo.one", "demo.two"]),
    ];
    assert_eq!(required_locales_complete("demo", &whole), Ok(()));

    let half = [
        locale("de", &["demo.one"]),
        english.clone(),
        locale("es", &["demo.one", "demo.two"]),
    ];
    let refused = required_locales_complete("demo", &half).expect_err("de lacks a code");
    assert!(
        refused.contains("locales/de.json lacks demo.two"),
        "{refused}"
    );
}

/// English alone is the documented minimum: a language the package does not ship falls back
/// to it. A language the interface does not require yet may carry a subset.
#[test]
fn a_missing_or_optional_language_is_not_a_gap() {
    assert_eq!(required_locales_complete("demo", &[]), Ok(()));
    let english = locale("en", &["demo.one", "demo.two"]);
    assert_eq!(
        required_locales_complete("demo", std::slice::from_ref(&english)),
        Ok(())
    );
    assert_eq!(
        required_locales_complete("demo", &[english, locale("it", &["demo.one"])]),
        Ok(())
    );
}

#[tokio::test]
async fn claiming_a_link_on_an_undeclared_host_fails_the_check() {
    assert_eq!(claims_no_foreign_link(async |_| Ok(false)).await, Ok(()));

    let refused = claims_no_foreign_link(async |url| Ok(url == FOREIGN_LINKS[1]))
        .await
        .expect_err("claims a foreign link");
    assert!(refused.contains(FOREIGN_LINKS[1]), "{refused}");

    let failed = claims_no_foreign_link(async |_| Err("claims-url failed: trap".to_owned())).await;
    assert_eq!(failed, Err("claims-url failed: trap".to_owned()));
}
