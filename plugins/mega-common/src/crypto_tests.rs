//! Recomputed against the public example the job file names, not quoted from anywhere.
//!
//! The file `yuZ0QJ6J` and its key come from the README of `justaprudev/pymegatools`, the
//! folder `e4diDZ7T` from `tonikelope/megabasterd` issue 215. Both are somebody else's
//! published example; no private link, no account and no real credential is in this tree.

use super::*;

/// The fragment key of the public example file.
const FILE_KEY: &str = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
/// Its `at` block, as the API answered on 2026-09-21 and again on 2026-09-22.
const FILE_AT: &str =
    "TKoehGVWQMtDWZz2bJxYF608OUaRdsuZ_FUtTWIo3Go4aCyL_5BXAvF7Vp4wge8uFCzGzOB18FN1QCVojMBj2w";
/// The share key of the public example folder.
const FOLDER_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn the_fragment_key_folds_into_the_three_measured_values() {
    let raw = b64_decode(FILE_KEY).expect("43 characters decode to 32 bytes");
    assert_eq!(raw.len(), 32);
    let key = FileKey::from_raw(&raw).expect("32 bytes fold");
    assert_eq!(hex(&key.key), "0c4c44e128eaee7a40bcbd4ffec19617");
    assert_eq!(hex(&key.nonce), "801b72fd9641ccfa");
    assert_eq!(hex(&key.meta_mac), "95ef644bf6dbda07");
    assert_eq!(hex(&key.mac_iv()), "801b72fd9641ccfa801b72fd9641ccfa");
}

#[test]
fn a_key_of_any_other_length_is_refused() {
    assert!(FileKey::from_raw(&[0; 16]).is_none());
    assert!(FileKey::from_raw(&[]).is_none());
}

#[test]
fn the_attribute_block_decrypts_to_the_measured_name_and_fingerprint() {
    let raw = b64_decode(FILE_KEY).expect("key");
    let key = FileKey::from_raw(&raw).expect("fold");
    let blob = b64_decode(FILE_AT).expect("attribute block");
    assert_eq!(blob.len(), 64);
    let plain = decrypt_attributes(&key.key, &blob).expect("MEGA prefix");
    assert_eq!(
        plain,
        r#"{"c":"08mHSeL9BYl-jnoGpUz2GwTWV65g","n":"10MB.bin"}"#
    );
    let attributes = Attributes::parse(&plain);
    assert_eq!(attributes.name.as_deref(), Some("10MB.bin"));
    // 2021-05-26 14:14:46 UTC, out of the fingerprint rather than out of the node's `ts`.
    assert_eq!(attributes.modified_at(), Some(1_622_038_486));
}

#[test]
fn a_block_under_the_wrong_key_has_no_mega_prefix() {
    let blob = b64_decode(FILE_AT).expect("attribute block");
    assert!(decrypt_attributes(&[0; 16], &blob).is_none());
}

#[test]
fn a_block_of_a_length_aes_cannot_read_is_refused() {
    assert!(decrypt_attributes(&[0; 16], &[]).is_none());
    assert!(decrypt_attributes(&[0; 16], &[0; 17]).is_none());
}

#[test]
fn the_folder_share_key_opens_the_measured_nodes() {
    let share: [u8; 16] = b64_decode(FOLDER_KEY)
        .expect("22 characters decode to 16 bytes")
        .try_into()
        .expect("16 bytes");
    // The folder node and the file node of the measured listing, with the key each carries
    // under the share root `G5NikTgR`.
    let folder_entry = "pR93bkC1OGslo_O5ugTeWw";
    let folder_attributes = "rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ";
    let raw = decrypt_node_key(&share, &b64_decode(folder_entry).expect("entry")).expect("key");
    assert_eq!(raw.len(), 16);
    let key: [u8; 16] = raw.try_into().expect("folder keys are used unfolded");
    let plain = decrypt_attributes(&key, &b64_decode(folder_attributes).expect("blob"))
        .expect("MEGA prefix");
    assert_eq!(
        Attributes::parse(&plain).name.as_deref(),
        Some("SharedFolder")
    );

    let file_entry = "IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q";
    let file_attributes =
        "2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag";
    let raw = decrypt_node_key(&share, &b64_decode(file_entry).expect("entry")).expect("key");
    assert_eq!(raw.len(), 32);
    let key = FileKey::from_raw(&raw).expect("fold");
    let plain =
        decrypt_attributes(&key.key, &b64_decode(file_attributes).expect("blob")).expect("prefix");
    let attributes = Attributes::parse(&plain);
    assert_eq!(attributes.name.as_deref(), Some("SharedFile.jpg"));
    assert!(attributes.fingerprint.is_some());
}

#[test]
fn old_links_with_the_standard_alphabet_still_decode() {
    let url_safe = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
    let standard = url_safe.replace('-', "+").replace('_', "/");
    assert_eq!(b64_decode(url_safe), b64_decode(&standard));
    assert_eq!(b64_decode("abc="), b64_decode("abc"));
}

#[test]
fn what_is_not_base64_is_not_a_key() {
    assert!(b64_decode("not a key!!").is_none());
}

#[test]
fn an_attribute_block_without_a_fingerprint_has_no_time() {
    let attributes = Attributes::parse(r#"{"n":"x"}"#);
    assert_eq!(attributes.modified_at(), None);
    assert_eq!(Attributes::parse("not json"), Attributes::default());
}

#[test]
fn encoding_round_trips_what_mega_sends() {
    let raw = b64_decode(FOLDER_KEY).expect("key");
    assert_eq!(b64_encode(&raw), FOLDER_KEY);
}
