//! The key canary for a file of the signed-in account (RD-120-30), after the pattern of
//! `key_canary.rs`.
//!
//! Three secrets are in play for an account file, and each has a place it may be and places it
//! must never reach. The **master key** -- the key half of the session -- is never in this
//! plugin at all: the host unwraps under it and hands back one file's key. The **session
//! identifier** is never a value here either: it leaves as the host's marker. The **file's own
//! key** exists exactly once, in the description's key field, which the host takes into its
//! vault type and never writes down.
//!
//! Every surface this plugin produces for an account file is rendered and searched for all
//! three in every spelling they could survive as: the address the crawler hands on, the
//! requests this plugin builds, the address the engine fetches from, the file name, every
//! failure it can produce, and the description's fields other than the key.
//!
//! No real account is in this tree. The account below is invented; the file is the public
//! example folder's, with its key re-wrapped under an invented master key.

use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
};
use mega_common::{Target, api, crypto};
use rd_plugin_mega::{messages, plan};

const OWNER: &str = "6W7cY6mgeJM";
const NODE: &str = "KlVgwR4B";
const SHARE_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";
/// The public example's node key, as its folder files it under the share key.
const SHARED_NODE_KEY: &str = "IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q";
const ATTRIBUTES: &str =
    "2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag";
const MASTER_KEY: [u8; 16] = *b"invented-master!";
const SESSION_ID: &str = "an-invented-session-identifier-of-43-chars";

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

/// The file's raw 32-byte node key: what the host's unwrap answers with.
fn raw_node_key() -> Vec<u8> {
    let share: [u8; 16] = crypto::b64_decode(SHARE_KEY)
        .and_then(|raw| raw.try_into().ok())
        .expect("a share key");
    ecb(
        &share,
        &crypto::b64_decode(SHARED_NODE_KEY).expect("key"),
        false,
    )
}

/// The account's node list, as MEGA would answer `a=f` for it.
fn listing() -> serde_json::Value {
    let wrapped = crypto::b64_encode(&ecb(&MASTER_KEY, &raw_node_key(), true));
    serde_json::json!({ "f": [
        { "h": NODE, "p": "RootNode", "u": OWNER, "t": 0, "a": ATTRIBUTES,
          "k": format!("{OWNER}:{wrapped}"), "s": 523_265 }
    ]})
}

fn answer() -> serde_json::Value {
    serde_json::json!({
        "s": 523_265, "at": ATTRIBUTES,
        "g": "https://gfs270n505.userstorage.mega.co.nz/dl/PLACEHOLDER"
    })
}

fn spellings(bytes: &[u8]) -> Vec<String> {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    vec![
        hex.to_uppercase(),
        bytes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        crypto::b64_encode(bytes),
        hex,
    ]
}

/// Every secret that must not surface, in every spelling.
fn canaries(file_key: &[u8]) -> Vec<String> {
    let mut all = spellings(&MASTER_KEY);
    all.extend(spellings(&raw_node_key()));
    all.extend(spellings(file_key));
    all.push(SESSION_ID.to_owned());
    all.push(String::from_utf8_lossy(&MASTER_KEY).into_owned());
    all
}

fn assert_clean(label: &str, rendered: &str, file_key: &[u8]) {
    for canary in canaries(file_key) {
        assert!(
            !rendered.contains(&canary),
            "{label}: a key survived as {canary:?} in: {rendered}"
        );
    }
}

fn described() -> plan::Description {
    let wrapped = plan::account_wrapped_key(&listing(), NODE).expect("the node's own key");
    // The host's side of the boundary: one AES step under the master key.
    let raw = ecb(&MASTER_KEY, &wrapped, false);
    let key = crypto::FileKey::from_raw(&raw).expect("a file key");
    plan::describe(&answer(), &key).expect("a description")
}

#[test]
fn the_address_the_crawler_hands_on_carries_no_key_and_no_session() {
    let description = described();
    let address = Target::account_file_url(NODE);
    assert_clean("account address", &address, &description.key);
    assert!(!address.contains('#'), "an account address has no fragment");
}

#[test]
fn the_requests_carry_the_sessions_marker_and_never_a_value() {
    let description = described();
    let (name, value) = api::SESSION_QUERY;
    assert_eq!(name, "sid");
    assert_eq!(value, "{{secret:mega_session}}");
    for (label, body) in [
        ("listing", api::account_listing_request()),
        ("download", api::account_file_request(NODE)),
    ] {
        assert_clean(label, &String::from_utf8_lossy(&body), &description.key);
    }
    assert_clean("endpoint", &api::endpoint(None), &description.key);
}

#[test]
fn what_leaves_for_the_host_to_unwrap_is_the_wrapped_key_and_never_the_master_key() {
    let wrapped = plan::account_wrapped_key(&listing(), NODE).expect("the node's own key");
    assert_eq!(wrapped.len(), 32);
    assert_ne!(wrapped, raw_node_key(), "the key went out unwrapped");
    assert!(
        !wrapped.windows(16).any(|window| window == MASTER_KEY),
        "the master key went out"
    );
}

#[test]
fn the_address_the_engine_fetches_from_and_the_file_name_carry_no_key() {
    let description = described();
    assert_clean("download address", &description.url, &description.key);
    let name = description.file_name.clone().expect("a name");
    assert_eq!(name, "SharedFile.jpg");
    assert_clean("file name", &name, &description.key);
    assert_clean(
        "nonce and integrity",
        &format!("{:?} {:?}", description.nonce, description.integrity),
        &description.key,
    );
}

#[test]
fn no_failure_an_account_file_can_produce_carries_a_key() {
    let description = described();
    let rendered: Vec<String> = [
        messages::ACCOUNT_NODE_MISSING,
        messages::ACCOUNT_KEY_FOREIGN,
        messages::KEY_INVALID,
        messages::SESSION_REQUIRED,
    ]
    .into_iter()
    .map(|(code, message)| format!("{code} {message}"))
    .chain(
        [
            plan::account_wrapped_key(&listing(), "NoSuchNd"),
            plan::account_wrapped_key(&serde_json::json!({ "f": [] }), NODE),
        ]
        .into_iter()
        .map(|result| format!("{:?}", result.expect_err("refused"))),
    )
    .collect();
    for line in rendered {
        assert_clean("failure", &line, &description.key);
    }
}

#[test]
fn a_key_filed_under_somebody_else_is_refused_by_name_and_not_sent_anywhere() {
    let mut foreign = listing();
    foreign["f"][0]["u"] = serde_json::Value::from("SomeoneElse");
    let refusal = plan::account_wrapped_key(&foreign, NODE).expect_err("not the account's own");
    assert_eq!(refusal.0, messages::ACCOUNT_KEY_FOREIGN);
}

/// The one place the file's key is meant to be, and it is the file's and not the account's.
#[test]
fn the_file_key_exists_once_and_is_the_one_the_public_folder_measured() {
    let description = described();
    assert_eq!(description.key.len(), 16);
    let public = crypto::FileKey::from_raw(&raw_node_key()).expect("a file key");
    assert_eq!(description.key, public.key.to_vec());
    assert_ne!(description.key, MASTER_KEY.to_vec());
}
