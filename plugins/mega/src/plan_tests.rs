//! Driven by answers recorded from MEGA on 2026-09-22 and sanitised: the two public examples
//! the job file names, no account, no private link, no real credential.

use super::*;

const FILE_KEY: &str = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
const FOLDER_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";

/// The `a=g` answer for the public example file, storage path replaced with a placeholder.
const FILE_ANSWER: &str = r#"{"s":10000000,"at":"TKoehGVWQMtDWZz2bJxYF608OUaRdsuZ_FUtTWIo3Go4aCyL_5BXAvF7Vp4wge8uFCzGzOB18FN1QCVojMBj2w","msd":1,"g":"https://gfs262n326.userstorage.mega.co.nz/dl/PLACEHOLDER","fh":"2fkJyP3kzJY"}"#;

const LISTING: &str = r#"{"f":[
 {"h":"G5NikTgR","p":"39FwkLpK","t":1,"a":"rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ","k":"G5NikTgR:pR93bkC1OGslo_O5ugTeWw","ts":1632475428},
 {"h":"KlVgwR4B","p":"G5NikTgR","t":0,"a":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","k":"G5NikTgR:IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q","s":523265,"ts":1632475461}
],"sn":"54_AmP_AxTw","noc":1}"#;

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is JSON")
}

#[test]
fn a_public_file_describes_its_name_size_and_whole_key_schedule() {
    let key = file_key(FILE_KEY).expect("fragment key");
    let description = describe(&json(FILE_ANSWER), &key).expect("description");
    assert_eq!(description.file_name.as_deref(), Some("10MB.bin"));
    assert_eq!(description.size, 10_000_000);
    assert_eq!(description.key.len(), 16);
    assert_eq!(description.nonce.len(), 8);
    assert!(description.url.contains(".userstorage.mega.co.nz/"));
    let integrity = description.integrity.expect("a file has an expected value");
    // The provider's own chunk layout, which is also the only layout parallel connections
    // may be split at.
    assert_eq!(integrity.boundaries.len(), 14);
    assert_eq!(*integrity.boundaries.last().expect("last"), 10_000_000);
    assert_eq!(integrity.iv.len(), 16);
    assert_eq!(integrity.expected.len(), 8);
    assert_eq!(integrity.iv[..8], integrity.iv[8..]);
}

#[test]
fn a_key_that_does_not_belong_to_the_file_is_refused_before_a_byte_is_fetched() {
    // The folder's share key, offered as a file key: it decodes, it folds, and the attribute
    // block turns to noise under it. That is the check.
    let wrong = FileKey::from_raw(&[7_u8; 32]).expect("32 bytes fold");
    let refusal = describe(&json(FILE_ANSWER), &wrong).expect_err("noise");
    assert_eq!(refusal.0, messages::ATTRIBUTES_UNREADABLE);
    assert_eq!(refusal.1, Category::Permanent);
}

#[test]
fn an_unreadable_fragment_is_a_refusal_and_not_a_panic() {
    assert_eq!(file_key("").expect_err("empty").0, messages::KEY_INVALID);
    assert_eq!(
        file_key(FOLDER_KEY).expect_err("too short").0,
        messages::KEY_INVALID
    );
    assert_eq!(
        share_key(FILE_KEY).expect_err("too long").0,
        messages::KEY_INVALID
    );
    assert!(share_key(FOLDER_KEY).is_ok());
}

#[test]
fn an_answer_without_an_address_is_not_a_description() {
    let key = file_key(FILE_KEY).expect("key");
    let refusal = describe(&json(r#"{"s":1,"at":""}"#), &key).expect_err("no address");
    assert_eq!(refusal.0, messages::INVALID_RESPONSE);
}

#[test]
fn an_empty_file_is_described_without_an_expected_value() {
    let key = file_key(FILE_KEY).expect("key");
    let answer = json(r#"{"s":0,"at":"","g":"https://gfs1n1.userstorage.mega.co.nz/dl/x"}"#);
    let description = describe(&answer, &key).expect("description");
    assert_eq!(description.size, 0);
    assert!(
        description.integrity.is_none(),
        "no chunk, nothing to condense"
    );
}

#[test]
fn a_folder_child_takes_its_key_out_of_the_listing() {
    let share = share_key(FOLDER_KEY).expect("share key");
    let key = node_key(&json(LISTING), "KlVgwR4B", &share).expect("node key");
    // The proof it is the right key: the node's own attribute block opens under it.
    let blob = mega_common::crypto::b64_decode(
        "2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag",
    )
    .expect("blob");
    let plain = mega_common::crypto::decrypt_attributes(&key.key, &blob).expect("prefix");
    assert_eq!(
        Attributes::parse(&plain).name.as_deref(),
        Some("SharedFile.jpg")
    );
}

#[test]
fn a_node_that_is_not_in_the_listing_is_a_refusal() {
    let share = share_key(FOLDER_KEY).expect("share key");
    assert_eq!(
        node_key(&json(LISTING), "notthere", &share)
            .expect_err("gone")
            .0,
        messages::NODE_MISSING
    );
    // A folder handle is not a file handle: the crawler names files, nothing else.
    assert_eq!(
        node_key(&json(LISTING), "G5NikTgR", &share)
            .expect_err("a folder")
            .0,
        messages::NODE_MISSING
    );
}

#[test]
fn the_measured_error_numbers_keep_their_meanings() {
    assert_eq!(api_refusal(-9).expect("known").0, messages::NOT_FOUND);
    assert_eq!(
        api_refusal(-15).expect("known").0,
        messages::SESSION_REQUIRED
    );
    assert_eq!(api_refusal(-4).expect("known").1, Category::RateLimited);
    assert_eq!(api_refusal(-17).expect("known").0, messages::QUOTA_EXCEEDED);
    assert_eq!(api_refusal(-18).expect("known").1, Category::Transient);
    assert_eq!(api_refusal(-2), None, "an unnamed number keeps its number");
}

#[test]
fn a_name_out_of_somebody_elses_attribute_block_cannot_be_a_path() {
    assert_eq!(
        clean_name(Some("../../etc/passwd")).as_deref(),
        Some("_.._etc_passwd")
    );
    assert_eq!(clean_name(Some("a\u{0}b")).as_deref(), Some("a b"));
    assert_eq!(clean_name(Some("   ")), None);
    assert_eq!(clean_name(Some("...")), None);
    assert_eq!(clean_name(None), None);
    assert_eq!(clean_name(Some("10MB.bin")).as_deref(), Some("10MB.bin"));
}
