//! Driven by the listing MEGA answered for the public example folder on 2026-09-22.

use super::*;

const FOLDER_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";

/// The measured answer: a root folder, a file in it, and an empty sub-folder.
const LISTING: &str = r#"{"f":[
 {"h":"G5NikTgR","p":"39FwkLpK","u":"6W7cY6mgeJM","t":1,"a":"rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ","k":"G5NikTgR:pR93bkC1OGslo_O5ugTeWw","ts":1632475428},
 {"h":"KlVgwR4B","p":"G5NikTgR","u":"6W7cY6mgeJM","t":0,"a":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","k":"G5NikTgR:IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q","s":523265,"fa":"882:0*Br9xX2hAYqw","ts":1632475461},
 {"h":"zwNiSB7J","p":"G5NikTgR","u":"6W7cY6mgeJM","t":1,"a":"bAMOwUGKrJzOHaWpoJEta9ATY54OrnrM1MdM18UevI4","k":"G5NikTgR:jRCDoNOtdI1WwR6-rOtbWg/zwNiSB7J:Gc71mTjFO44hyqSjItIL9g","ts":1632475524}
],"sn":"54_AmP_AxTw","noc":1}"#;

fn share() -> [u8; 16] {
    mega_common::crypto::b64_decode(FOLDER_KEY)
        .and_then(|raw| <[u8; 16]>::try_from(raw).ok())
        .expect("22 characters are a share key")
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is JSON")
}

#[test]
fn the_measured_folder_yields_its_one_file_with_name_size_and_place() {
    let found = expand(&json(LISTING), &share()).expect("one file");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].node, "KlVgwR4B");
    assert_eq!(found[0].name, "SharedFile.jpg");
    assert_eq!(found[0].size, 523_265);
    // The shared folder's own name is the outermost segment, which is what the host reads as
    // the package suggestion.
    assert_eq!(found[0].path, "SharedFolder");
}

#[test]
fn a_folder_whose_files_none_of_this_keys_opens_is_empty_rather_than_wrong() {
    assert_eq!(expand(&json(LISTING), &[0; 16]), Err(Refusal::Empty));
}

#[test]
fn an_answer_with_no_nodes_has_no_root() {
    assert_eq!(expand(&json(r#"{"f":[]}"#), &share()), Err(Refusal::NoRoot));
    assert_eq!(
        expand(&json(r#"{"sn":"x"}"#), &share()),
        Err(Refusal::NoRoot)
    );
}

#[test]
fn a_name_out_of_somebody_elses_attribute_block_cannot_be_a_path() {
    assert_eq!(clean(Some("../etc")).as_deref(), Some("_etc"));
    assert_eq!(clean(Some("a\u{0}b")).as_deref(), Some("a b"));
    assert_eq!(clean(Some("  . ")), None);
    assert_eq!(clean(None), None);
}

/// The file limit is the host's `MAX_CRAWLED_LINKS`, not a number of this plugin's own. It
/// cannot be imported here -- a guest links nothing outside `rdownloader:plugin` -- so it is
/// written down, and `mega_contract.rs` holds the two against each other over the real
/// component, where the host's constant is in scope.
#[test]
fn the_limits_are_the_ones_the_manifest_was_written_for() {
    assert_eq!(MAX_FILES, 1_000);
    assert_eq!(MAX_DEPTH, 32);
}
