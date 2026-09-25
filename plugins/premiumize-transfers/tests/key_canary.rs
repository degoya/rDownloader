//! The key canary for Premiumize transfers (RD-120-23), after the pattern of
//! `plugins/mega/tests/key_canary.rs`.
//!
//! "The API key never appears in an address, a log or an error message" is an acceptance
//! criterion, and an acceptance criterion that rests on everybody remembering not to print
//! something is not one.
//!
//! This plugin's position is stronger than MEGA's: it never holds the key at all. The host
//! substitutes `{{secret:premiumize_api_key}}` on the way out, so the only way a key could
//! leak from here is if the plugin asked for it somewhere other than that template, or if it
//! forwarded something the provider sent back. Both are checked below, the second by feeding
//! a key-shaped string through every surface that takes provider text.
//!
//! The wire is proven separately and more strongly in
//! `crates/rd-plugin-ext/tests/premiumize_transfers_contract.rs`, which reads the headers the
//! component actually produced.

use rd_plugin_premiumize_transfers::{api, messages};

/// A key-shaped string, in the places a provider answer could carry one.
const CANARY: &str = "pmkey1234567890abcdefABCDEF0987654321";

/// The package directory, read when the test *runs* rather than when it compiles.
///
/// `env!` bakes the path in at compile time, and the worktrees share one target directory while
/// `plugins/` is deliberately never stamped (that would make every component look stale). So a
/// test binary built in one worktree is reused by the next, still carrying the first one's path
/// -- and once that worktree is removed, this test fails with "No such file or directory" in a
/// branch that never touched it. It did, on 2026-09-23. The test runner sets the variable at run
/// time for the checkout actually being tested.
fn manifest_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .expect("the test runner sets CARGO_MANIFEST_DIR")
}

fn sources() -> Vec<String> {
    let directory = manifest_dir();
    vec![
        std::fs::read_to_string(directory.join("src/guest.rs")).expect("the guest source"),
        std::fs::read_to_string(directory.join("src/api.rs")).expect("the api source"),
    ]
}

/// The reference is named in exactly one place and only ever inside a `{{secret:…}}`
/// template. A request that named it any other way would be asking for the value.
#[test]
fn the_reference_is_only_ever_named_inside_a_secret_template() {
    assert_eq!(api::KEY_REFERENCE, "premiumize_api_key");
    let guest = &sources()[0];
    let named = guest.matches("KEY_REFERENCE").count();
    assert_eq!(
        named, 1,
        "the reference is named {named} times in the guest"
    );
    assert!(
        guest.contains(r#"format!("Bearer {{{{secret:{}}}}}", api::KEY_REFERENCE)"#),
        "the one naming is not the bearer template"
    );
    for source in sources() {
        assert!(
            !source.contains(CANARY),
            "a key-shaped literal is committed in a source"
        );
    }
}

/// Whatever the provider wrote, it does not come back out. The sentence is classified and
/// dropped; only its own stable code travels.
#[test]
fn a_provider_sentence_carrying_a_key_is_classified_and_never_forwarded() {
    for (code, message) in [
        (None, Some(format!("Not logged in: {CANARY}"))),
        (
            Some("authentication_failed"),
            Some(format!("bad key {CANARY}")),
        ),
        (
            Some("unknown_code_nobody_has_seen"),
            Some(CANARY.to_owned()),
        ),
    ] {
        let refusal = api::refusal(code, message.as_deref(), None);
        assert!(!refusal.message.contains(CANARY), "{refusal:?}");
        assert!(!refusal.code.contains(CANARY), "{refusal:?}");
        assert_eq!(
            refusal.api_code.as_deref(),
            code,
            "the stable code travels and nothing else does"
        );
        assert!(
            refusal
                .api_code
                .as_deref()
                .is_none_or(|api_code| !api_code.contains(CANARY))
        );
    }
}

/// Every text this plugin can print is one it wrote itself, and none of them names a secret.
#[test]
fn no_message_this_plugin_can_print_names_a_secret() {
    for (code, message) in [
        messages::AUTH_INVALID,
        messages::API_ERROR,
        messages::CONTAINER_UNKNOWN,
        messages::HTTP_ERROR,
        messages::LIMIT_REACHED,
        messages::NO_CHOICE,
        messages::NO_FILES,
        messages::NO_LOCATION,
        messages::NO_TRANSFER_ID,
        messages::NOT_A_SOURCE,
        messages::RATE_LIMITED,
        messages::SERVER_BUSY,
        messages::SOURCE_UNSUPPORTED,
        messages::TRANSFER_FAILED,
        messages::TRANSFER_GONE,
        messages::TRANSFER_UNLISTED,
    ] {
        assert!(code.starts_with("premiumize_transfers."), "{code}");
        for text in [code, message] {
            assert!(!text.contains("secret"), "{text}");
            assert!(!text.contains(api::KEY_REFERENCE), "{text}");
            assert!(!text.contains("{{"), "{text}");
        }
    }
    assert!(!messages::http_error(503).contains(api::KEY_REFERENCE));
}

/// The two request bodies carry what a person handed over and nothing else. A source that
/// happened to contain a key-shaped string still travels -- it is the person's address, not a
/// credential -- but nothing beside it does.
#[test]
fn a_request_body_carries_the_source_and_nothing_else() {
    let body =
        String::from_utf8(api::form_body("src", "https://example.invalid/a")).expect("ascii");
    assert_eq!(body, "src=https%3A%2F%2Fexample.invalid%2Fa");
    assert!(!body.contains("secret"));
    let boundary = api::boundary_for("abcdef");
    let multipart = String::from_utf8(
        api::multipart_body(&boundary, "source.dlc", b"PAYLOAD").expect("a body"),
    )
    .expect("ascii");
    assert!(!multipart.contains("secret"), "{multipart}");
    assert!(!multipart.contains(api::KEY_REFERENCE), "{multipart}");
}
