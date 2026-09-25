//! An account folder built from the public example (RD-120-30).
//!
//! No real account is in this tree. The listing below is the measured public folder with one
//! change: every key is re-filed under an invented owner handle and re-wrapped under an
//! invented master key, the way MEGA files the keys of an account's own nodes. The names and
//! the attribute blocks are the measured ones, so the names this walk reads are real
//! decryptions and not strings a test wrote down.

use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
};
use mega_common::crypto;
use serde_json::Value;

use super::{Refusal, expand};

const SHARE_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";
const OWNER: &str = "6W7cY6mgeJM";
const MASTER_KEY: [u8; 16] = *b"invented-master!";

/// `(handle, parent, kind, attributes, key under the share, size)` of the measured folder.
const NODES: [(&str, &str, u8, &str, &str, u64); 3] = [
    (
        "G5NikTgR",
        "39FwkLpK",
        1,
        "rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ",
        "pR93bkC1OGslo_O5ugTeWw",
        0,
    ),
    (
        "KlVgwR4B",
        "G5NikTgR",
        0,
        "2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag",
        "IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q",
        523_265,
    ),
    (
        "zwNiSB7J",
        "G5NikTgR",
        1,
        "bAMOwUGKrJzOHaWpoJEta9ATY54OrnrM1MdM18UevI4",
        "jRCDoNOtdI1WwR6-rOtbWg",
        0,
    ),
];

fn ecb(key: &[u8; 16], data: &[u8], encrypt: bool) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let mut out = data.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        if encrypt {
            cipher.encrypt_block(block.into());
        } else {
            cipher.decrypt_block(block.into());
        }
    }
    out
}

/// The measured folder, as an account would list it: keys under the owner, wrapped under the
/// master key.
fn account_listing() -> Value {
    let share: [u8; 16] = crypto::b64_decode(SHARE_KEY)
        .and_then(|raw| raw.try_into().ok())
        .expect("a share key");
    let nodes: Vec<Value> = NODES
        .iter()
        .map(|(handle, parent, kind, attributes, key, size)| {
            let raw = ecb(&share, &crypto::b64_decode(key).expect("key"), false);
            let wrapped = crypto::b64_encode(&ecb(&MASTER_KEY, &raw, true));
            serde_json::json!({
                "h": handle, "p": parent, "u": OWNER, "t": kind, "a": attributes,
                "k": format!("{OWNER}:{wrapped}"), "s": size,
            })
        })
        .collect();
    serde_json::json!({ "f": nodes })
}

/// What the host does in the component: one AES step under the master key.
fn host_unwrap(wrapped: &[u8]) -> Result<Vec<u8>, &'static str> {
    Ok(ecb(&MASTER_KEY, wrapped, false))
}

#[test]
fn an_account_folder_yields_its_file_with_the_name_its_own_key_opens() {
    let mut asked = Vec::new();
    let found = expand(&account_listing(), "G5NikTgR", |wrapped: &[u8]| {
        asked.push(wrapped.len());
        host_unwrap(wrapped)
    })
    .expect("one file");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].node, "KlVgwR4B");
    assert_eq!(found[0].name, "SharedFile.jpg");
    assert_eq!(found[0].size, 523_265);
    assert_eq!(found[0].path, "SharedFolder");
    // Every key went to the host, one node at a time, and never the master key the other
    // way: the closure is the only way this module has to open anything.
    assert_eq!(asked, vec![16, 32, 16]);
}

#[test]
fn a_single_file_of_the_account_is_its_own_answer_without_a_path() {
    let found = expand(&account_listing(), "KlVgwR4B", host_unwrap).expect("the file");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "SharedFile.jpg");
    assert_eq!(found[0].path, "");
}

#[test]
fn a_node_that_is_not_in_the_account_is_named_as_such() {
    assert_eq!(
        expand(&account_listing(), "NoSuchNd", host_unwrap),
        Err(Refusal::NotInAccount)
    );
}

#[test]
fn a_key_filed_under_somebody_else_is_skipped_rather_than_sent_to_the_host() {
    // A node that reached the account through a share files its key under that share's
    // handle. The account's master key does not open it, and the host is not asked to try.
    let mut listing = account_listing();
    for node in listing["f"].as_array_mut().expect("nodes") {
        node["u"] = Value::from("SomeoneElse");
    }
    let mut asked = 0;
    let result = expand(&listing, "G5NikTgR", |wrapped: &[u8]| {
        asked += 1;
        host_unwrap(wrapped)
    });
    assert_eq!(result, Err(Refusal::Empty));
    assert_eq!(asked, 0);
}

#[test]
fn a_host_refusal_is_carried_through_rather_than_read_as_an_empty_folder() {
    let result = expand(&account_listing(), "G5NikTgR", |_: &[u8]| {
        Err::<Vec<u8>, _>("plugin.provider_secret_missing")
    });
    assert_eq!(result, Err(Refusal::Host("plugin.provider_secret_missing")));
}
