//! The key canary for MEGA (RD-103-02), after the pattern of `rd-http`'s (RD-110-33).
//!
//! "Decryption keys never appear in logs or UI URLs" is an acceptance criterion, and an
//! acceptance criterion that rests on everybody remembering not to print something is not one.
//! `rd-http` proved it for the host's side of the boundary; this proves it for the plugin's,
//! which is the side that derives the key in the first place.
//!
//! Every surface this plugin produces is rendered and searched for the key in every spelling
//! it could survive as: the address handed to the engine, the file name written to the row,
//! the failure messages, and the `Debug` of the description itself.

use rd_plugin_mega::{messages, plan};

/// The public example the job file names, from somebody else's README. No private link.
const FRAGMENT: &str = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
const ANSWER: &str = r#"{"s":10000000,"at":"TKoehGVWQMtDWZz2bJxYF608OUaRdsuZ_FUtTWIo3Go4aCyL_5BXAvF7Vp4wge8uFCzGzOB18FN1QCVojMBj2w","g":"https://gfs262n326.userstorage.mega.co.nz/dl/PLACEHOLDER"}"#;

/// Every spelling the derived key could survive as, plus the fragment it came from.
fn canaries(key: &[u8]) -> Vec<String> {
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    vec![
        hex.clone(),
        hex.to_uppercase(),
        key.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        mega_common::crypto::b64_encode(key),
        FRAGMENT.to_owned(),
    ]
}

fn assert_clean(label: &str, rendered: &str, key: &[u8]) {
    for canary in canaries(key) {
        assert!(
            !rendered.contains(&canary),
            "{label}: the key survived as {canary:?} in: {rendered}"
        );
    }
}

fn described() -> plan::Description {
    let key = plan::file_key(FRAGMENT).expect("the fragment folds");
    let answer = serde_json::from_str(ANSWER).expect("fixture is JSON");
    plan::describe(&answer, &key).expect("a description")
}

#[test]
fn the_address_the_engine_fetches_from_carries_no_key() {
    let description = described();
    assert_clean("download address", &description.url, &description.key);
    // And it is a MEGA storage address rather than anything else.
    assert!(description.url.contains(".userstorage.mega.co.nz/"));
}

#[test]
fn the_file_name_written_to_the_row_carries_no_key() {
    let description = described();
    let name = description.file_name.clone().expect("a name");
    assert_eq!(name, "10MB.bin");
    assert_clean("file name", &name, &description.key);
}

#[test]
fn no_failure_this_plugin_can_produce_carries_a_key() {
    let description = described();
    // Every refusal the plugin knows, rendered the way the interface would.
    let rendered: Vec<String> = [-9_i64, -11, -15, -3, -4, -17, -16, -18]
        .into_iter()
        .filter_map(plan::api_refusal)
        .map(|((code, message), category)| format!("{code} {message} {category:?}"))
        .chain([
            format!("{:?}", messages::KEY_INVALID),
            format!("{:?}", messages::ATTRIBUTES_UNREADABLE),
            format!("{:?}", messages::NOT_MINE),
            messages::api_error(-9),
            messages::http_error(503),
        ])
        .collect();
    for line in rendered {
        assert_clean("failure", &line, &description.key);
    }
}

#[test]
fn a_refusal_derived_from_an_unreadable_fragment_does_not_quote_it() {
    let refusal = plan::file_key("not-a-key").expect_err("refused");
    let rendered = format!("{refusal:?}");
    assert!(!rendered.contains("not-a-key"), "{rendered}");
}

/// The one place the key is *meant* to be: the description's own field, which is what the
/// host takes straight into `TransformKey` and never writes down.
#[test]
fn the_key_exists_exactly_once_and_is_the_right_one() {
    let description = described();
    assert_eq!(description.key.len(), 16);
    assert_eq!(
        description
            .key
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "0c4c44e128eaee7a40bcbd4ffec19617",
        "the measured file key, recomputed"
    );
    // The condensed value the host checks the plaintext against is the provider's, not ours.
    let integrity = description.integrity.expect("an expected value");
    assert_eq!(
        integrity
            .expected
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "95ef644bf6dbda07"
    );
}
